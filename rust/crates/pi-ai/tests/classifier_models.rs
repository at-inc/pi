//! Port of `packages/ai/test/classifier-models.test.ts`.
//!
//! `rejects chat models at the classifier entry point at runtime` casts a chat model to `ClassifierModel`; Rust cannot
//! cast, so the test builds a `ClassifierModel` from the chat model's fields with `type: "chat"`.

use std::cell::RefCell;
use std::rc::Rc;

use indexmap::IndexMap;
use pi_ai::auth::credential_store::InMemoryCredentialStore;
use pi_ai::auth::types::{
	ApiKeyAuth, ApiKeyCredential, AuthResult, Credential, CredentialStore, OAuthCredential, ProviderAuth,
};
use pi_ai::models::{CreateModelsOptions, CreateProviderOptions, ProviderApi, create_models, create_provider};
use pi_ai::providers::all::{
	builtin_models, get_all_builtin_models, get_builtin_classifier_model, get_builtin_classifier_models,
};
use pi_ai::types::{
	AnyModel, ClassifierContext, ClassifierModel, ClassifierOptions, ClassifierResult, ClassifierStopReason, Model,
	ModelType, ProviderClassifier, ProviderStreamOptions, ProviderStreams, SimpleStreamOptions, TranscriptContext,
};
use pi_ai::utils::event_stream::AssistantMessageEventStream;
use pi_ai::utils::model_operations::get_model_type;
use pi_host::{LocalBoxFuture, PiResult};
use serde_json::{Value, json};

/// `expect(actual).toMatchObject(expected)` for JSON values; numbers compare as `f64`, like JS numbers.
fn assert_match_object(actual: &Value, expected: &Value) {
	match (actual, expected) {
		(Value::Object(actual), Value::Object(expected)) => {
			for (key, expected) in expected {
				assert_match_object(actual.get(key).unwrap_or_else(|| panic!("missing key {key}")), expected);
			}
		}
		(Value::Array(actual), Value::Array(expected)) => {
			assert_eq!(actual.len(), expected.len());
			for (actual, expected) in actual.iter().zip(expected) {
				assert_match_object(actual, expected);
			}
		}
		(Value::Number(actual), Value::Number(expected)) => assert_eq!(actual.as_f64(), expected.as_f64()),
		_ => assert_eq!(actual, expected),
	}
}

fn classifier_model(provider: &str, id: &str) -> ClassifierModel {
	serde_json::from_value(json!({
		"type": "classifier",
		"id": id,
		"name": id,
		"api": "test-classifier",
		"provider": provider,
		"baseUrl": "https://example.test/v1",
		"input": ["text"],
		"cost": { "input": 0, "output": 0, "cacheRead": 0, "cacheWrite": 0 },
		"contextWindow": 1000,
	}))
	.unwrap()
}

fn chat_model_json(provider: &str, id: &str) -> Value {
	json!({
		"id": id,
		"name": id,
		"api": "test-chat",
		"provider": provider,
		"baseUrl": "https://example.test/v1",
		"reasoning": false,
		"input": ["text"],
		"cost": { "input": 0, "output": 0, "cacheRead": 0, "cacheWrite": 0 },
		"contextWindow": 1000,
		"maxTokens": 100,
	})
}

fn chat_model(provider: &str, id: &str) -> Model {
	serde_json::from_value(chat_model_json(provider, id)).unwrap()
}

fn context() -> ClassifierContext {
	serde_json::from_value(json!({
		"state": { "text": "yes" },
		"questions": {
			"approved": {
				"type": "bool",
				"instructions": "Does this express approval?",
				"criteria": { "true": "Approval", "false": "No approval" },
			},
		},
	}))
	.unwrap()
}

fn context_with_images(images: Value) -> ClassifierContext {
	let mut context = to_json(context());
	context["images"] = images;
	serde_json::from_value(context).unwrap()
}

fn to_json(value: impl serde::Serialize) -> Value {
	serde_json::to_value(value).unwrap()
}

fn no_auth() -> ProviderAuth {
	ProviderAuth {
		api_key: Some(ApiKeyAuth {
			name: "Test".into(),
			login: None,
			check: None,
			resolve: Rc::new(|_| Box::pin(async { Ok(Some(AuthResult::default())) })),
		}),
		oauth: None,
	}
}

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

/// A classifier that answers `answers` and counts its calls.
struct FixedClassifier {
	answers: Value,
	calls: Rc<RefCell<usize>>,
}

impl ProviderClassifier for FixedClassifier {
	fn classify(
		&self,
		model: &ClassifierModel,
		_context: &ClassifierContext,
		_options: Option<ClassifierOptions>,
	) -> LocalBoxFuture<'static, PiResult<ClassifierResult>> {
		*self.calls.borrow_mut() += 1;
		let result = ClassifierResult {
			api: model.api.clone(),
			provider: model.provider.clone(),
			model: model.id.clone(),
			answers: serde_json::from_value(self.answers.clone()).unwrap(),
			usage: None,
			stop_reason: ClassifierStopReason::Stop,
			error_message: None,
			timestamp: pi_host::now_ms(),
		};
		Box::pin(async move { Ok(result) })
	}
}

fn fixed_classifier(answers: Value, calls: Rc<RefCell<usize>>) -> IndexMap<String, Rc<dyn ProviderClassifier>> {
	IndexMap::from([(
		"test-classifier".to_string(),
		Rc::new(FixedClassifier { answers, calls }) as Rc<dyn ProviderClassifier>,
	)])
}

mod models_with_classifier_models {
	use super::*;

	#[test]
	fn keeps_chat_and_classifier_entries_with_the_same_provider_and_id_separate() {
		pi_host::testing::run(|_| async {
			let chat = chat_model("test", "shared");
			let classifier = classifier_model("test", "shared");
			let provider = create_provider(CreateProviderOptions {
				id: "test".into(),
				auth: no_auth(),
				models: vec![AnyModel::Chat(chat), AnyModel::Classifier(classifier.clone())],
				api: Some(ProviderApi::ByApi(IndexMap::from([(
					"test-chat".to_string(),
					Rc::new(EmptyStreams) as Rc<dyn ProviderStreams>,
				)]))),
				classifiers: Some(fixed_classifier(
					json!({ "approved": { "type": "bool", "probability": 0.9 } }),
					Rc::default(),
				)),
				..CreateProviderOptions::default()
			})
			.unwrap();
			let models = create_models(None);
			models.set_provider(Rc::new(provider));

			let listed_chat = models.get_model("test", "shared").unwrap();
			assert_eq!(get_model_type(&listed_chat), "chat");
			assert_eq!(
				to_json(models.get_model_of_type(ModelType::Classifier, "test", "shared"))["type"],
				json!("classifier")
			);
			assert_eq!(
				models.get_models_of_type(ModelType::Classifier, None),
				vec![AnyModel::Classifier(classifier.clone())]
			);
			assert_eq!(models.get_all_models(None).len(), 2);
			assert_eq!(
				models
					.get_available_of_type(ModelType::Classifier, None, None)
					.await
					.unwrap(),
				vec![AnyModel::Classifier(classifier.clone())]
			);
			assert_eq!(
				to_json(&models.classify(&classifier, &context(), None).await.answers["approved"]),
				json!({ "type": "bool", "probability": 0.9 })
			);
		});
	}

	#[test]
	fn rejects_chat_models_at_the_classifier_entry_point_at_runtime() {
		pi_host::testing::run(|_| async {
			let chat = chat_model("test", "chat");
			let models = create_models(None);
			models.set_provider(Rc::new(
				create_provider(CreateProviderOptions {
					id: "test".into(),
					auth: no_auth(),
					models: vec![AnyModel::Chat(chat)],
					api: Some(ProviderApi::Single(Rc::new(EmptyStreams))),
					..CreateProviderOptions::default()
				})
				.unwrap(),
			));
			let mut as_classifier = chat_model_json("test", "chat");
			as_classifier["type"] = json!("chat");
			let as_classifier: ClassifierModel = serde_json::from_value(as_classifier).unwrap();

			let result = models.classify(&as_classifier, &context(), None).await;
			assert_eq!(result.stop_reason, ClassifierStopReason::Error);
			assert!(result.error_message.unwrap().contains("is not a classifier model"));
		});
	}

	#[test]
	fn exposes_jev_only_through_classifier_catalog_accessors() {
		let jev = get_builtin_classifier_model("typesafe", "jev-latest").unwrap();
		assert_match_object(
			&to_json(&jev),
			&json!({
				"type": "classifier",
				"api": "typesafe-system-one",
				"provider": "typesafe",
				"contextWindow": 64000,
			}),
		);
		assert_eq!(get_builtin_classifier_models("typesafe"), vec![jev.clone()]);
		assert_eq!(
			get_all_builtin_models("typesafe"),
			vec![AnyModel::Classifier(jev.clone())]
		);

		let models = builtin_models(None);
		assert_eq!(models.get_model("typesafe", "jev-latest"), None);
		assert_eq!(
			models.get_model_of_type(ModelType::Classifier, "typesafe", "jev-latest"),
			Some(AnyModel::Classifier(jev))
		);
	}

	#[test]
	fn routes_jev_to_its_typesafe_compatible_endpoint() {
		for (provider, id, url) in [
			(
				"vercel-ai-gateway",
				"typesafe-ai/jev",
				"https://ai-gateway.vercel.sh/typesafe/v1/systemone",
			),
			(
				"opencode",
				"jev-1.13",
				"https://opencode.ai/inference/systemone/v1/systemone",
			),
			(
				"opencode",
				"jev-1.13-free",
				"https://opencode.ai/inference/systemone/v1/systemone",
			),
		] {
			pi_host::testing::run(|_| async move {
				let models = builtin_models(None);
				let jev = models
					.get_model_of_type(ModelType::Classifier, provider, id)
					.unwrap_or_else(|| panic!("missing {provider} Jev model"));
				assert_match_object(
					&to_json(&jev),
					&json!({ "api": "typesafe-system-one", "contextWindow": 32000 }),
				);
				assert_eq!(models.get_model(provider, id), None, "{provider}/{id}");
				let AnyModel::Classifier(jev) = jev else {
					panic!("{provider}/{id} is not a classifier");
				};

				type Recorded = (String, Value, Option<String>);
				let requests: Rc<RefCell<Vec<Recorded>>> = Rc::new(RefCell::new(Vec::new()));
				let recorded = requests.clone();
				let fetch: pi_host::FetchFunction = Rc::new(move |request: pi_host::HttpRequest| {
					recorded.borrow_mut().push((
						request.url.clone(),
						serde_json::from_slice(request.body.as_deref().unwrap()).unwrap(),
						request.header("authorization").map(str::to_string),
					));
					let body = json!({ "model": id, "answers": { "approved": { "type": "noul", "noul": 0.8 } } });
					Box::pin(async move {
						Ok(pi_host::HttpResponse::from_bytes(
							200,
							IndexMap::from([("content-type".to_string(), "application/json".to_string())]),
							pi_host::js::json_stringify(&body),
						))
					})
				});
				let result = models
					.classify(
						&jev,
						&context(),
						Some(pi_ai::models::ModelsClassifierOptions {
							base: ClassifierOptions {
								base: pi_ai::types::ProviderRequestOptions {
									api_key: Some("secret".into()),
									fetch: Some(fetch),
									..Default::default()
								},
								temperature: None,
							},
							..Default::default()
						}),
					)
					.await;

				let requests = requests.borrow();
				assert_eq!(requests.len(), 1);
				let (request_url, body, authorization) = &requests[0];
				assert_eq!(request_url, url);
				assert_match_object(body, &json!({ "model": id, "state": to_json(context())["state"] }));
				assert_eq!(authorization.as_deref(), Some("Bearer secret"));
				assert_eq!(result.stop_reason, ClassifierStopReason::Stop);
				assert_eq!(
					to_json(&result.answers["approved"]),
					json!({ "type": "bool", "probability": 0.8 })
				);
			});
		}
	}

	#[test]
	fn rejects_images_for_classifier_models_without_image_input_before_calling_the_provider() {
		pi_host::testing::run(|_| async {
			let classifier = classifier_model("test", "text-only");
			let calls = Rc::new(RefCell::new(0));
			let models = create_models(None);
			models.set_provider(Rc::new(
				create_provider(CreateProviderOptions {
					id: "test".into(),
					auth: no_auth(),
					models: vec![AnyModel::Classifier(classifier.clone())],
					classifiers: Some(fixed_classifier(json!({}), calls.clone())),
					..CreateProviderOptions::default()
				})
				.unwrap(),
			));

			let result = models
				.classify(
					&classifier,
					&context_with_images(json!([{ "type": "image", "data": "aW1hZ2U=", "mimeType": "image/png" }])),
					None,
				)
				.await;
			let without_images = models
				.classify(&classifier, &context_with_images(json!([])), None)
				.await;

			assert_eq!(result.stop_reason, ClassifierStopReason::Error);
			assert_eq!(
				result.error_message.as_deref(),
				Some("Model test/text-only does not accept image input")
			);
			assert_eq!(without_images.stop_reason, ClassifierStopReason::Stop);
			assert_eq!(*calls.borrow(), 1);
		});
	}

	#[test]
	fn routes_openai_gpt_6_luna_through_the_decisions_api_with_images() {
		pi_host::testing::run(|_| async {
			let models = builtin_models(None);
			let luna = models
				.get_model_of_type(ModelType::Classifier, "openai", "gpt-6-luna")
				.expect("missing OpenAI Decisions model");
			assert_match_object(
				&to_json(&luna),
				&json!({ "api": "openai-decisions", "input": ["text", "image"], "contextWindow": 922000 }),
			);
			// The chat entry with the same id stays separate.
			assert_eq!(
				models.get_model("openai", "gpt-6-luna").unwrap().api,
				"openai-responses"
			);
			let AnyModel::Classifier(luna) = luna else {
				panic!("gpt-6-luna is not a classifier");
			};

			let urls: Rc<RefCell<Vec<String>>> = Rc::new(RefCell::new(Vec::new()));
			let recorded = urls.clone();
			let fetch: pi_host::FetchFunction = Rc::new(move |request: pi_host::HttpRequest| {
				recorded.borrow_mut().push(request.url.clone());
				let body = json!({ "answers": [{ "type": "predicate", "name": "approved", "probability": 0.8 }] });
				Box::pin(async move {
					Ok(pi_host::HttpResponse::from_bytes(
						200,
						IndexMap::from([("content-type".to_string(), "application/json".to_string())]),
						pi_host::js::json_stringify(&body),
					))
				})
			});
			let result = models
				.classify(
					&luna,
					&context_with_images(json!([{ "type": "image", "data": "aW1hZ2U=", "mimeType": "image/png" }])),
					Some(pi_ai::models::ModelsClassifierOptions {
						base: ClassifierOptions {
							base: pi_ai::types::ProviderRequestOptions {
								api_key: Some("secret".into()),
								fetch: Some(fetch),
								..Default::default()
							},
							temperature: None,
						},
						..Default::default()
					}),
				)
				.await;

			assert_eq!(*urls.borrow(), ["https://api.openai.com/v1/decisions"]);
			assert_eq!(
				result.stop_reason,
				ClassifierStopReason::Stop,
				"{:?}",
				result.error_message
			);
			assert_eq!(
				to_json(&result.answers["approved"]),
				json!({ "type": "bool", "probability": 0.8 })
			);
		});
	}

	#[test]
	fn lists_openai_decisions_models_only_for_api_key_credentials() {
		pi_host::testing::run(|_| async {
			let api_key_store = Rc::new(InMemoryCredentialStore::new());
			api_key_store
				.modify(
					"openai",
					Box::new(|_| {
						Box::pin(async {
							Ok(Some(Credential::ApiKey(ApiKeyCredential {
								key: Some("secret".into()),
								env: None,
								extra: Default::default(),
							})))
						})
					}),
					None,
				)
				.await
				.unwrap();
			let oauth_store = Rc::new(InMemoryCredentialStore::new());
			let expires = pi_host::now_ms() + 3_600_000.0;
			oauth_store
				.modify(
					"openai",
					Box::new(move |_| {
						Box::pin(async move {
							Ok(Some(Credential::OAuth(OAuthCredential {
								access: "access".into(),
								refresh: "refresh".into(),
								expires,
								..OAuthCredential::default()
							})))
						})
					}),
					None,
				)
				.await
				.unwrap();

			let with_api_key = builtin_models(Some(CreateModelsOptions {
				credentials: Some(api_key_store),
				..CreateModelsOptions::default()
			}));
			let with_oauth = builtin_models(Some(CreateModelsOptions {
				credentials: Some(oauth_store),
				..CreateModelsOptions::default()
			}));

			let ids = |models: Vec<AnyModel>| {
				models
					.iter()
					.map(|model| to_json(model)["id"].clone())
					.collect::<Vec<_>>()
			};
			assert_eq!(
				ids(with_api_key
					.get_available_of_type(ModelType::Classifier, Some("openai"), None)
					.await
					.unwrap()),
				[json!("gpt-6-luna")]
			);
			assert_eq!(
				with_oauth
					.get_available_of_type(ModelType::Classifier, Some("openai"), None)
					.await
					.unwrap(),
				vec![]
			);
			// Chat models stay available with ChatGPT OAuth.
			assert!(
				with_oauth
					.get_available(Some("openai"), None)
					.await
					.unwrap()
					.iter()
					.any(|model| model.id == "gpt-6-luna")
			);
		});
	}

	#[test]
	fn routes_openrouter_classifier_models_through_the_system_one_api() {
		let models = builtin_models(None);
		for model in get_builtin_classifier_models("openrouter") {
			assert_match_object(
				&to_json(&model),
				&json!({
					"api": "typesafe-system-one",
					"baseUrl": "https://openrouter.ai/api/v1",
					// Explicit, so images keep OpenRouter's form when the baseUrl is overridden.
					"compat": { "imageInput": "state-image-urls", "sendImageOptInHeader": false },
				}),
			);
			assert_eq!(models.get_model("openrouter", &model.id), None);
			assert_eq!(
				models.get_model_of_type(ModelType::Classifier, "openrouter", &model.id),
				Some(AnyModel::Classifier(model.clone()))
			);
		}
	}
}
