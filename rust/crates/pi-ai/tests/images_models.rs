//! Port of `packages/ai/test/images-models.test.ts`.
//!
//! Differences from the TS:
//! - Every test that resolves provider auth (`getAuth`, `getAvailable*`, `refresh`, `generateImages` past the provider
//!   lookup) is `#[ignore = "needs w2-ai-auth: ..."]`: `Models` resolves auth through the `resolve_provider_auth` stub
//!   on this branch, and `getAvailable*` and `refresh` read the default `InMemoryCredentialStore` stub first. They
//!   pass against the auth branch.
//! - "keeps existing built-in and compat model reads chat-only" does not check `getModels` from `compat.ts`
//!   (`expect(compat).toEqual(chat)`): `compat.rs` belongs to another shard and is still pending. The rest of the test
//!   runs against `get_builtin_models`.
//! - The `as unknown as ImageModel` and `as unknown as Model` casts are models of the other struct whose `type` is the
//!   original model's: an `ImageModel` with `type: "chat"`, and a `Model` with `type: "image"`.
//! - `hasApi` returns the chat model or `None` instead of a type-guard boolean.
//! - The `GenerateCall` list records the `ProviderImagesOptions` the provider received; `toBeUndefined` on a model
//!   lookup is `None`.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;

use indexmap::{IndexMap, IndexSet};
use pi_ai::auth::resolve::AuthResolutionOverrides;
use pi_ai::auth::types::{ApiKeyAuth, AuthContext, AuthResult, ModelAuth, ProviderAuth};
use pi_ai::models::{
	CreateModelsOptions, CreateProviderOptions, GetAuthTarget, Models, ModelsImagesOptions, ModelsRefreshOptions,
	Provider, ProviderApi, create_models, create_provider, get_model_type, has_api, is_model_type,
};
use pi_ai::models_store::{InMemoryModelsStore, ModelsStore};
use pi_ai::providers::all::{
	builtin_models, get_all_builtin_models, get_builtin_classifier_models, get_builtin_image_model,
	get_builtin_image_models, get_builtin_models,
};
use pi_ai::types::{
	AnyModel, AssistantImages, BaseModel, Context, ImageContent, ImageModel, ImagesContext, ImagesOptions,
	ImagesStopReason, Model, ModelCost, ModelInputModality, ModelType, ProviderImages, ProviderImagesOptions,
	ProviderRequestOptions, ProviderStreamOptions, ProviderStreams, SimpleStreamOptions, StopReason, TextContent,
	TranscriptContext, UserContent,
};
use pi_ai::utils::event_stream::AssistantMessageEventStream;
use pi_ai::utils::model_operations::ModelRef;
use pi_host::testing::run;
use pi_host::{AbortController, LocalBoxFuture, PiResult};
use serde_json::json;

/// `fakeAuthContext(env)`.
struct FakeAuthContext {
	env: HashMap<String, String>,
}

impl AuthContext for FakeAuthContext {
	fn env(&self, name: &str) -> LocalBoxFuture<'static, Option<String>> {
		let value = self.env.get(name).cloned();
		Box::pin(async move { value })
	}

	fn file_exists(&self, _path: &str) -> LocalBoxFuture<'static, bool> {
		Box::pin(async { false })
	}
}

fn fake_auth_context(env: &[(&str, &str)]) -> Option<Rc<dyn AuthContext>> {
	Some(Rc::new(FakeAuthContext {
		env: env
			.iter()
			.map(|(name, value)| (name.to_string(), value.to_string()))
			.collect(),
	}))
}

fn image_model(provider: &str, id: &str) -> ImageModel {
	ImageModel {
		base: BaseModel {
			id: id.into(),
			name: id.into(),
			api: "test-images".into(),
			provider: provider.into(),
			base_url: "https://example.test/v1".into(),
			input: vec![ModelInputModality::Text],
			cost: ModelCost::default(),
			..BaseModel::default()
		},
		r#type: ModelType::Image,
		output: vec![ModelInputModality::Image],
		extra: Default::default(),
	}
}

fn chat_model(provider: &str, id: &str) -> Model {
	Model {
		base: BaseModel {
			id: id.into(),
			name: id.into(),
			api: "test-chat".into(),
			provider: provider.into(),
			base_url: "https://example.test/v1".into(),
			input: vec![ModelInputModality::Text],
			cost: ModelCost::default(),
			..BaseModel::default()
		},
		reasoning: false,
		context_window: 1000.0,
		max_tokens: 100.0,
		..Model::default()
	}
}

fn ok_result(model: &ImageModel) -> AssistantImages {
	AssistantImages {
		api: model.api.clone(),
		provider: model.provider.clone(),
		model: model.id.clone(),
		output: vec![UserContent::Image(ImageContent {
			data: "aGk=".into(),
			mime_type: "image/png".into(),
			..ImageContent::default()
		})],
		response_id: None,
		usage: None,
		stop_reason: ImagesStopReason::Stop,
		error_message: None,
		timestamp: pi_host::now_ms(),
	}
}

struct GenerateCall {
	model: ImageModel,
	options: Option<ProviderImagesOptions>,
}

type Calls = Rc<RefCell<Vec<GenerateCall>>>;

/// `{ generateImages }`: records each call and returns `okResult(model)`.
struct RecordingImages {
	calls: Option<Calls>,
}

impl ProviderImages for RecordingImages {
	fn generate_images(
		&self,
		model: &ImageModel,
		_context: &ImagesContext,
		options: Option<ProviderImagesOptions>,
	) -> LocalBoxFuture<'static, PiResult<AssistantImages>> {
		if let Some(calls) = &self.calls {
			calls.borrow_mut().push(GenerateCall {
				model: model.clone(),
				options,
			});
		}
		let result = ok_result(model);
		Box::pin(async move { Ok(result) })
	}
}

/// `{ stream: () => new AssistantMessageEventStream(), streamSimple: ... }`.
struct EmptyStreams;

impl ProviderStreams for EmptyStreams {
	fn stream(
		&self,
		_model: &Model,
		_context: &TranscriptContext,
		_options: Option<ProviderStreamOptions>,
	) -> PiResult<AssistantMessageEventStream> {
		Ok(AssistantMessageEventStream::new())
	}

	fn stream_simple(
		&self,
		_model: &Model,
		_context: &TranscriptContext,
		_options: Option<SimpleStreamOptions>,
	) -> PiResult<AssistantMessageEventStream> {
		Ok(AssistantMessageEventStream::new())
	}
}

/// `{ apiKey: { name, resolve: async () => ({ auth: {} }) } }`.
fn ambient_auth(name: &str) -> ProviderAuth {
	ProviderAuth {
		api_key: Some(ApiKeyAuth {
			name: name.into(),
			login: None,
			check: None,
			resolve: Rc::new(|_| {
				Box::pin(async {
					Ok(Some(AuthResult {
						auth: ModelAuth::default(),
						..AuthResult::default()
					}))
				})
			}),
		}),
		oauth: None,
	}
}

#[derive(Default)]
struct TestProviderInput {
	id: &'static str,
	models: Option<Vec<AnyModel>>,
	env_var: Option<&'static str>,
	calls: Option<Calls>,
	images: Option<Vec<&'static str>>,
}

fn test_provider(input: TestProviderInput) -> Rc<Provider> {
	let env_var = input.env_var;
	let images = input
		.images
		.unwrap_or_else(|| vec!["test-images"])
		.into_iter()
		.map(|api| {
			let images: Rc<dyn ProviderImages> = Rc::new(RecordingImages {
				calls: input.calls.clone(),
			});
			(api.to_string(), images)
		})
		.collect();
	let api: Rc<dyn ProviderStreams> = Rc::new(EmptyStreams);
	Rc::new(
		create_provider(CreateProviderOptions {
			id: input.id.into(),
			auth: ProviderAuth {
				api_key: Some(ApiKeyAuth {
					name: "Test key".into(),
					login: None,
					check: None,
					resolve: Rc::new(move |input| {
						Box::pin(async move {
							let Some(env_var) = env_var else {
								return Ok(Some(AuthResult::default()));
							};
							let stored = input.credential.as_ref().and_then(|credential| credential.key.clone());
							let key = match stored {
								Some(key) => Some(key),
								None => input.ctx.env(env_var).await,
							};
							Ok(key.map(|key| AuthResult {
								auth: ModelAuth {
									api_key: Some(key),
									..ModelAuth::default()
								},
								source: Some(if input.credential.is_some() {
									"stored".into()
								} else {
									env_var.into()
								}),
								..AuthResult::default()
							}))
						})
					}),
				}),
				oauth: None,
			},
			models: input
				.models
				.unwrap_or_else(|| vec![AnyModel::Image(image_model(input.id, "model-a"))]),
			api: Some(ProviderApi::ByApi(IndexMap::from([("test-chat".to_string(), api)]))),
			images: Some(images),
			..CreateProviderOptions::default()
		})
		.expect("the test provider is valid"),
	)
}

fn context() -> ImagesContext {
	ImagesContext {
		input: vec![UserContent::Text(TextContent {
			text: "a red circle".into(),
			..TextContent::default()
		})],
	}
}

fn ids(models: &[AnyModel]) -> Vec<&str> {
	models.iter().map(|model| model.model_id()).collect()
}

fn image_of(model: Option<AnyModel>) -> ImageModel {
	match model {
		Some(AnyModel::Image(model)) => model,
		other => panic!("expected an image model, got {other:?}"),
	}
}

fn api_of(model: &AnyModel) -> &str {
	match model {
		AnyModel::Chat(model) => &model.api,
		AnyModel::Image(model) => &model.api,
		AnyModel::Classifier(model) => &model.api,
		AnyModel::Other(_) => panic!("unknown model type"),
	}
}

fn images_options(configure: impl FnOnce(&mut ModelsImagesOptions)) -> Option<ModelsImagesOptions> {
	let mut options = ModelsImagesOptions::default();
	configure(&mut options);
	Some(options)
}

async fn api_key(
	models: &dyn Models,
	target: GetAuthTarget<'_>,
	overrides: Option<AuthResolutionOverrides>,
) -> Option<String> {
	models
		.get_auth(target, overrides)
		.await
		.unwrap()
		.and_then(|result| result.auth.api_key)
}

mod model_discriminants {
	use super::*;

	#[test]
	fn treats_models_without_a_type_as_chat_models() {
		let chat = chat_model("p", "c");
		let image = image_model("p", "i");

		assert_eq!(chat.r#type, None);
		assert_eq!(get_model_type(&chat), "chat");
		assert_eq!(
			get_model_type(&Model {
				r#type: Some(ModelType::Chat),
				..chat.clone()
			}),
			"chat"
		);
		assert_eq!(get_model_type(&image), "image");
		assert!(is_model_type(&chat, ModelType::Chat));
		assert!(!is_model_type(&chat, ModelType::Image));
		assert!(is_model_type(&image, ModelType::Image));

		// hasApi never matches an image model, even on an equal api string
		let mut renamed = image.clone();
		renamed.base.api = "test-chat".into();
		assert!(has_api(&AnyModel::Image(renamed), "test-chat").is_none());
		assert!(has_api(&AnyModel::Chat(chat), "test-chat").is_some());
	}
}

mod models_with_image_models {
	use super::*;

	#[test]
	#[ignore = "needs w2-ai-auth: InMemoryCredentialStore, resolve_provider_auth"]
	fn lists_models_without_a_type_as_chat_models_at_create_provider_boundaries() {
		run(|_| async {
			let api: Rc<dyn ProviderStreams> = Rc::new(EmptyStreams);
			let provider = Rc::new(
				create_provider(CreateProviderOptions {
					id: "legacy".into(),
					auth: ambient_auth("Test"),
					models: vec![
						AnyModel::Chat(chat_model("legacy", "static")),
						AnyModel::Image(image_model("legacy", "static")),
					],
					fetch_models: Some(Rc::new(|_| {
						Box::pin(async { Ok(vec![AnyModel::Chat(chat_model("legacy", "dynamic"))]) })
					})),
					api: Some(ProviderApi::Single(api)),
					..CreateProviderOptions::default()
				})
				.unwrap(),
			);
			let models = create_models(None);
			models.set_provider(provider.clone());

			let listed = |provider: &Provider| (provider.get_models)();
			assert_eq!(
				listed(&provider)
					.iter()
					.map(|model| model.id.as_str())
					.collect::<Vec<_>>(),
				["static"]
			);
			models
				.refresh(Some(ModelsRefreshOptions {
					providers: Some(vec![provider.id.clone()]),
					..ModelsRefreshOptions::default()
				}))
				.await
				.unwrap();
			assert_eq!(
				listed(&provider)
					.iter()
					.map(|model| model.id.as_str())
					.collect::<Vec<_>>(),
				["static", "dynamic"]
			);
			assert_eq!(
				listed(&provider).iter().map(|model| model.r#type).collect::<Vec<_>>(),
				[None, None]
			);
			assert_eq!(
				ids(&models.get_models_of_type(ModelType::Image, Some("legacy"))),
				["static"]
			);
		});
	}

	#[test]
	fn lists_chat_image_and_all_models_through_typed_accessors() {
		run(|_| async {
			let models = create_models(None);
			models.set_provider(test_provider(TestProviderInput {
				id: "p1",
				models: Some(vec![
					AnyModel::Chat(chat_model("p1", "c1")),
					AnyModel::Image(image_model("p1", "i1")),
					AnyModel::Image(image_model("p1", "i2")),
				]),
				..TestProviderInput::default()
			}));
			models.set_provider(test_provider(TestProviderInput {
				id: "p2",
				models: Some(vec![AnyModel::Image(image_model("p2", "i3"))]),
				..TestProviderInput::default()
			}));

			assert_eq!(
				models
					.get_models(None)
					.iter()
					.map(|m| m.id.as_str())
					.collect::<Vec<_>>(),
				["c1"]
			);
			assert_eq!(ids(&models.get_models_of_type(ModelType::Chat, None)), ["c1"]);
			assert_eq!(
				ids(&models.get_models_of_type(ModelType::Image, None)),
				["i1", "i2", "i3"]
			);
			assert_eq!(
				ids(&models.get_models_of_type(ModelType::Image, Some("p1"))),
				["i1", "i2"]
			);
			assert_eq!(models.get_models_of_type(ModelType::Classifier, None), vec![]);
			assert_eq!(ids(&models.get_all_models(None)), ["c1", "i1", "i2", "i3"]);

			assert_eq!(models.get_model("p1", "c1").map(|m| m.id.clone()), Some("c1".into()));
			assert_eq!(models.get_model("p1", "i1"), None);
			assert_eq!(
				models
					.get_model_of_type(ModelType::Chat, "p1", "c1")
					.map(|m| m.model_id().to_string()),
				Some("c1".into())
			);
			assert_eq!(
				models
					.get_model_of_type(ModelType::Image, "p1", "i1")
					.map(|m| m.model_id().to_string()),
				Some("i1".into())
			);
			assert_eq!(models.get_model_of_type(ModelType::Image, "p1", "c1"), None);
		});
	}

	#[test]
	#[ignore = "needs w2-ai-auth: InMemoryCredentialStore, resolve_provider_auth"]
	fn splits_available_models_by_type() {
		run(|_| async {
			let models = create_models(Some(CreateModelsOptions {
				auth_context: fake_auth_context(&[("KEY", "k")]),
				..CreateModelsOptions::default()
			}));
			models.set_provider(test_provider(TestProviderInput {
				id: "p1",
				env_var: Some("KEY"),
				models: Some(vec![
					AnyModel::Chat(chat_model("p1", "c1")),
					AnyModel::Image(image_model("p1", "i1")),
				]),
				..TestProviderInput::default()
			}));
			models.set_provider(test_provider(TestProviderInput {
				id: "p2",
				env_var: Some("MISSING"),
				models: Some(vec![AnyModel::Image(image_model("p2", "i2"))]),
				..TestProviderInput::default()
			}));

			assert_eq!(
				models
					.get_available(None, None)
					.await
					.unwrap()
					.iter()
					.map(|m| m.id.as_str())
					.collect::<Vec<_>>(),
				["c1"]
			);
			assert_eq!(
				ids(&models.get_available_of_type(ModelType::Chat, None, None).await.unwrap()),
				["c1"]
			);
			assert_eq!(
				ids(&models
					.get_available_of_type(ModelType::Image, None, None)
					.await
					.unwrap()),
				["i1"]
			);
			assert_eq!(ids(&models.get_all_available(None, None).await.unwrap()), ["c1", "i1"]);
		});
	}

	#[test]
	#[ignore = "needs w2-ai-auth: resolve_provider_auth"]
	fn resolves_auth_through_the_provider_and_merges_it_into_image_requests_explicit_options_win() {
		run(|_| async {
			let calls: Calls = Rc::default();
			let models = create_models(Some(CreateModelsOptions {
				auth_context: fake_auth_context(&[("TEST_KEY", "env-key")]),
				..CreateModelsOptions::default()
			}));
			models.set_provider(test_provider(TestProviderInput {
				id: "p1",
				env_var: Some("TEST_KEY"),
				calls: Some(calls.clone()),
				..TestProviderInput::default()
			}));
			let model = image_of(models.get_model_of_type(ModelType::Image, "p1", "model-a"));
			let target = AnyModel::Image(model.clone());

			assert_eq!(
				api_key(&*models, GetAuthTarget::Model(&target), None).await,
				Some("env-key".into())
			);
			assert_eq!(
				api_key(&*models, GetAuthTarget::Provider(&model.provider), None).await,
				Some("env-key".into())
			);
			assert_eq!(
				api_key(
					&*models,
					GetAuthTarget::Model(&target),
					Some(AuthResolutionOverrides {
						api_key: Some("explicit-key".into()),
						..AuthResolutionOverrides::default()
					})
				)
				.await,
				Some("explicit-key".into())
			);

			let result = models.generate_images(&model, &context(), None).await;
			assert_eq!(result.stop_reason, ImagesStopReason::Stop);
			assert_eq!(
				calls.borrow()[0]
					.options
					.as_ref()
					.and_then(|options| options.api_key.clone()),
				Some("env-key".into())
			);

			models
				.generate_images(
					&model,
					&context(),
					images_options(|options| options.base.base.api_key = Some("explicit".into())),
				)
				.await;
			assert_eq!(
				calls.borrow()[1]
					.options
					.as_ref()
					.and_then(|options| options.api_key.clone()),
				Some("explicit".into())
			);
			assert_eq!(calls.borrow()[1].model, model);
		});
	}

	#[test]
	#[ignore = "needs w2-ai-auth: resolve_provider_auth"]
	fn merges_provider_resolved_env_and_applies_header_transforms() {
		run(|_| async {
			let calls: Calls = Rc::default();
			let models = create_models(None);
			let images: Rc<dyn ProviderImages> = Rc::new(RecordingImages {
				calls: Some(calls.clone()),
			});
			models.set_provider(Rc::new(
				create_provider(CreateProviderOptions {
					id: "p1".into(),
					auth: ProviderAuth {
						api_key: Some(ApiKeyAuth {
							name: "Test key".into(),
							login: None,
							check: None,
							resolve: Rc::new(|_| {
								Box::pin(async {
									Ok(Some(AuthResult {
										auth: ModelAuth {
											api_key: Some("provider-key".into()),
											headers: Some(IndexMap::from([(
												"x-base".to_string(),
												Some("1".to_string()),
											)])),
											..ModelAuth::default()
										},
										env: Some(IndexMap::from([
											("PROVIDER_ONLY".to_string(), "provider".to_string()),
											("SHARED".to_string(), "provider".to_string()),
										])),
										..AuthResult::default()
									}))
								})
							}),
						}),
						oauth: None,
					},
					models: vec![AnyModel::Image(image_model("p1", "model-a"))],
					images: Some(IndexMap::from([("test-images".to_string(), images)])),
					..CreateProviderOptions::default()
				})
				.unwrap(),
			));
			let model = image_of(models.get_model_of_type(ModelType::Image, "p1", "model-a"));

			models
				.generate_images(
					&model,
					&context(),
					Some(ModelsImagesOptions {
						base: ImagesOptions {
							base: ProviderRequestOptions {
								api_key: Some("request-key".into()),
								env: Some(IndexMap::from([
									("REQUEST_ONLY".to_string(), "request".to_string()),
									("SHARED".to_string(), "request".to_string()),
								])),
								..ProviderRequestOptions::default()
							},
							metadata: None,
						},
						transform_headers: Some(Rc::new(|mut headers| {
							headers.insert("x-extra".into(), Some("2".into()));
							Box::pin(async move { Ok(headers) })
						})),
					}),
				)
				.await;

			let calls = calls.borrow();
			let options = calls[0].options.as_ref().unwrap();
			assert_eq!(options.api_key.as_deref(), Some("request-key"));
			assert_eq!(
				serde_json::to_value(&options.env).unwrap(),
				json!({ "PROVIDER_ONLY": "provider", "REQUEST_ONLY": "request", "SHARED": "request" })
			);
			assert_eq!(
				serde_json::to_value(&options.headers).unwrap(),
				json!({ "x-base": "1", "x-extra": "2" })
			);
		});
	}

	#[test]
	#[ignore = "needs w2-ai-auth: resolve_provider_auth"]
	fn returns_error_results_instead_of_rejecting() {
		run(|_| async {
			let models = create_models(Some(CreateModelsOptions {
				auth_context: fake_auth_context(&[]),
				..CreateModelsOptions::default()
			}));

			let ghost = models
				.generate_images(&image_model("ghost", "m"), &context(), None)
				.await;
			assert_eq!(ghost.stop_reason, ImagesStopReason::Error);
			assert!(ghost.error_message.unwrap().contains("Unknown provider: ghost"));

			// Unconfigured auth is an error, matching stream().
			let calls: Calls = Rc::default();
			models.set_provider(test_provider(TestProviderInput {
				id: "p1",
				env_var: Some("MISSING"),
				calls: Some(calls.clone()),
				..TestProviderInput::default()
			}));
			let model = image_of(models.get_model_of_type(ModelType::Image, "p1", "model-a"));
			assert_eq!(
				models
					.get_auth(GetAuthTarget::Model(&AnyModel::Image(model.clone())), None)
					.await
					.unwrap(),
				None
			);
			let unconfigured = models.generate_images(&model, &context(), None).await;
			assert_eq!(unconfigured.stop_reason, ImagesStopReason::Error);
			assert!(unconfigured.error_message.unwrap().contains("not configured"));
			assert!(calls.borrow().is_empty());

			let controller = AbortController::new();
			controller.abort();
			let cancelled = models
				.generate_images(
					&model,
					&context(),
					images_options(|options| options.base.base.signal = Some(controller.signal())),
				)
				.await;
			assert_eq!(cancelled.stop_reason, ImagesStopReason::Aborted);
			assert!(calls.borrow().is_empty());

			// A provider without any images implementation rejects image models it lists.
			let api: Rc<dyn ProviderStreams> = Rc::new(EmptyStreams);
			models.set_provider(Rc::new(
				create_provider(CreateProviderOptions {
					id: "chat-only".into(),
					auth: ambient_auth("Test"),
					models: vec![AnyModel::Image(image_model("chat-only", "i"))],
					api: Some(ProviderApi::Single(api)),
					..CreateProviderOptions::default()
				})
				.unwrap(),
			));
			let unsupported = models
				.generate_images(
					&image_of(models.get_model_of_type(ModelType::Image, "chat-only", "i")),
					&context(),
					None,
				)
				.await;
			assert_eq!(unsupported.stop_reason, ImagesStopReason::Error);
			assert!(
				unsupported
					.error_message
					.unwrap()
					.contains("does not support image generation")
			);

			// An images map without the model's api yields a provider error result.
			models.set_provider(test_provider(TestProviderInput {
				id: "wrong-api",
				models: Some(vec![AnyModel::Image(image_model("wrong-api", "i"))]),
				images: Some(vec!["other-images"]),
				..TestProviderInput::default()
			}));
			let missing_api = models
				.generate_images(
					&image_of(models.get_model_of_type(ModelType::Image, "wrong-api", "i")),
					&context(),
					None,
				)
				.await;
			assert_eq!(missing_api.stop_reason, ImagesStopReason::Error);
			assert!(
				missing_api
					.error_message
					.unwrap()
					.contains("no image generation implementation for \"test-images\"")
			);
		});
	}

	#[test]
	#[ignore = "needs w2-ai-auth: resolve_provider_auth"]
	fn builtin_models_registers_the_built_in_image_providers() {
		run(|_| async {
			let models = builtin_models(Some(CreateModelsOptions {
				auth_context: fake_auth_context(&[("OPENROUTER_API_KEY", "or-key")]),
				..CreateModelsOptions::default()
			}));
			let images = models.get_models_of_type(ModelType::Image, None);
			assert_eq!(
				images
					.iter()
					.map(|model| model.model_provider())
					.collect::<IndexSet<_>>()
					.into_iter()
					.collect::<Vec<_>>(),
				["openai-codex", "openrouter"]
			);
			let openrouter = models.get_models_of_type(ModelType::Image, Some("openrouter"));
			assert!(!openrouter.is_empty());
			assert!(openrouter.iter().all(|model| api_of(model) == "openrouter-images"));
			assert_eq!(
				api_key(&*models, GetAuthTarget::Model(&openrouter[0]), None).await,
				Some("or-key".into())
			);
			assert_eq!(
				models
					.get_model_of_type(ModelType::Image, "openai-codex", "chatgpt-image-generation")
					.as_ref()
					.map(api_of),
				Some("openai-codex-images")
			);
		});
	}

	#[test]
	fn rejects_chat_models_at_the_image_entry_point_at_runtime() {
		run(|_| async {
			let models = create_models(None);
			let chat = chat_model("p1", "chat");
			models.set_provider(test_provider(TestProviderInput {
				id: "p1",
				models: Some(vec![AnyModel::Chat(chat.clone())]),
				images: Some(vec!["test-chat"]),
				..TestProviderInput::default()
			}));

			let cast = ImageModel {
				base: BaseModel {
					api: (*chat.api).into(),
					..image_model("p1", "chat").base
				},
				r#type: ModelType::Chat,
				..image_model("p1", "chat")
			};
			let result = models.generate_images(&cast, &context(), None).await;
			assert_eq!(result.stop_reason, ImagesStopReason::Error);
			assert!(result.error_message.unwrap().contains("is not an image model"));
		});
	}

	#[test]
	fn rejects_image_models_at_the_stream_entry_points_at_runtime() {
		run(|_| async {
			let models = create_models(None);
			models.set_provider(test_provider(TestProviderInput {
				id: "p1",
				..TestProviderInput::default()
			}));
			let image = image_of(models.get_model_of_type(ModelType::Image, "p1", "model-a"));

			let cast = Model {
				base: BaseModel {
					id: image.id.clone(),
					name: image.name.clone(),
					api: (*image.api).into(),
					provider: image.provider.clone(),
					base_url: image.base_url.clone(),
					input: image.input.clone(),
					cost: image.cost.clone(),
					..BaseModel::default()
				},
				r#type: Some(ModelType::Image),
				..Model::default()
			};
			let result = models
				.stream_simple(&cast, &Context::default(), None)
				.result()
				.await
				.unwrap();
			assert_eq!(result.stop_reason, StopReason::Error);
			assert!(result.error_message.unwrap().contains("is not a chat model"));
		});
	}

	#[test]
	fn requires_at_least_one_concrete_operation_implementation() {
		let create_empty_provider = |implementations: CreateProviderOptions| {
			create_provider(CreateProviderOptions {
				id: "empty".into(),
				auth: ambient_auth("Test"),
				models: Vec::new(),
				..implementations
			})
		};

		let message = "at least one of \"api\", \"images\", or \"classifiers\"";
		for implementations in [
			CreateProviderOptions::default(),
			CreateProviderOptions {
				api: Some(ProviderApi::ByApi(IndexMap::new())),
				..CreateProviderOptions::default()
			},
			CreateProviderOptions {
				images: Some(IndexMap::new()),
				..CreateProviderOptions::default()
			},
			CreateProviderOptions {
				classifiers: Some(IndexMap::new()),
				..CreateProviderOptions::default()
			},
		] {
			let Err(error) = create_empty_provider(implementations) else {
				panic!("createProvider accepted a provider without implementations");
			};
			assert!(error.message.contains(message), "{}", error.message);
		}
	}

	#[test]
	#[ignore = "needs w2-ai-auth: InMemoryCredentialStore, resolve_provider_auth"]
	fn supports_dynamic_providers_listing_image_models_via_refresh() {
		run(|_| async {
			let fetches = Rc::new(Cell::new(0));
			let models_store = Rc::new(InMemoryModelsStore::new());
			let models = create_models(Some(CreateModelsOptions {
				models_store: Some(models_store.clone()),
				..CreateModelsOptions::default()
			}));
			let images: Rc<dyn ProviderImages> = Rc::new(RecordingImages { calls: None });
			let counted = fetches.clone();
			models.set_provider(Rc::new(
				create_provider(CreateProviderOptions {
					id: "dyn".into(),
					auth: ambient_auth("Test"),
					models: Vec::new(),
					fetch_models: Some(Rc::new(move |_| {
						counted.set(counted.get() + 1);
						Box::pin(async {
							Ok(vec![
								AnyModel::Image(image_model("dyn", "listed")),
								AnyModel::Chat(chat_model("dyn", "chat")),
							])
						})
					})),
					images: Some(IndexMap::from([("test-images".to_string(), images)])),
					..CreateProviderOptions::default()
				})
				.unwrap(),
			));

			assert_eq!(models.get_all_models(Some("dyn")), vec![]);
			let result = models
				.refresh(Some(ModelsRefreshOptions {
					providers: Some(vec!["dyn".into()]),
					..ModelsRefreshOptions::default()
				}))
				.await
				.unwrap();
			assert_eq!(result.errors.len(), 0);
			assert_eq!(fetches.get(), 1);
			assert!(models.get_model_of_type(ModelType::Image, "dyn", "listed").is_some());
			assert!(models.get_model("dyn", "chat").is_some());
			let stored = models_store.read("dyn", None).await.unwrap();
			assert_eq!(
				stored.map(|entry| ids(&entry.models).join(",")),
				Some("listed,chat".into())
			);
		});
	}

	#[test]
	fn keeps_existing_built_in_and_compat_model_reads_chat_only() {
		let chat = get_builtin_models("openrouter");
		let images = get_builtin_image_models("openrouter");
		let all = get_all_builtin_models("openrouter");

		assert!(chat.iter().all(|model| is_model_type(model, ModelType::Chat)));
		assert!(images.iter().all(|model| is_model_type(model, ModelType::Image)));
		assert!(all.iter().any(|model| is_model_type(model, ModelType::Image)));
		assert!(chat.iter().all(|model| model.context_window > 0.0));
		assert_eq!(
			chat.len() + images.len() + get_builtin_classifier_models("openrouter").len(),
			all.len()
		);
		assert_eq!(
			get_builtin_image_model("openrouter", "black-forest-labs/flux.2-pro").map(|model| model.r#type),
			Some(ModelType::Image)
		);
	}

	#[test]
	#[ignore = "needs w2-ai-auth: resolve_provider_auth"]
	fn builtin_models_exposes_openrouter_image_models_under_the_openrouter_provider() {
		run(|_| async {
			let models = builtin_models(Some(CreateModelsOptions {
				auth_context: fake_auth_context(&[("OPENROUTER_API_KEY", "or-key")]),
				..CreateModelsOptions::default()
			}));
			let provider = models.get_provider("openrouter").unwrap();
			let images = models.get_models_of_type(ModelType::Image, Some("openrouter"));
			assert!(!images.is_empty());
			assert!(
				(provider.get_models)()
					.iter()
					.all(|model| is_model_type(model, ModelType::Chat))
			);
			assert!(provider.get_all_models.as_ref().is_some_and(|get_all_models| {
				get_all_models()
					.iter()
					.any(|model| is_model_type(model, ModelType::Image))
			}));
			assert!(
				images
					.iter()
					.all(|m| is_model_type(m, ModelType::Image) && api_of(m) == "openrouter-images")
			);
			assert!(
				models
					.get_models_of_type(ModelType::Image, None)
					.iter()
					.all(|m| m.model_provider() == "openai-codex" || m.model_provider() == "openrouter")
			);

			// One upstream id can expose separate chat and image operations.
			let chat = models.get_model("openrouter", "google/gemini-3-pro-image");
			let image = models.get_model_of_type(ModelType::Image, "openrouter", "google/gemini-3-pro-image");
			assert_eq!(chat.as_ref().map(|chat| &*chat.api), Some("openai-completions"));
			assert_eq!(image.as_ref().map(api_of), Some("openrouter-images"));

			// One credential covers both.
			assert_eq!(
				api_key(&*models, GetAuthTarget::Model(&images[0]), None).await,
				Some("or-key".into())
			);
			let chat = AnyModel::Chat(chat.unwrap());
			assert_eq!(
				api_key(&*models, GetAuthTarget::Model(&chat), None).await,
				Some("or-key".into())
			);
			assert!(provider.generate_images.is_some());
		});
	}
}
