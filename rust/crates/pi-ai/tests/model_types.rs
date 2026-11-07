//! Port of `packages/ai/test/model-types.test.ts`.
//!
//! The handwritten provider of `work through a handwritten provider without getAllModels` is `faux_provider`'s
//! provider with `get_all_models` removed and `get_models` returning `faux.models`. `return model shapes that can be
//! reassigned within one api` is a compile-time check in TS; Rust getters return plain `Model`s, so only its value
//! checks remain. `getModel` from `compat.ts` is a re-export of `get_builtin_model` (`compat::get_model`).

use std::cell::RefCell;
use std::rc::Rc;

use pi_ai::auth::types::{ApiKeyAuth, AuthResult, ProviderAuth};
use pi_ai::compat::get_model as get_compat_model;
use pi_ai::models::{
	CreateModelsOptions, CreateProviderOptions, ModelsRefreshOptions, Provider, ProviderApi, create_models,
	create_provider, get_model_type, has_api, is_model_type, models_are_equal,
};
use pi_ai::models_store::{InMemoryModelsStore, ModelsStore, ModelsStoreEntry};
use pi_ai::providers::all::get_builtin_model;
use pi_ai::providers::faux::{RegisterFauxProviderOptions, faux_assistant_message, faux_provider};
use pi_ai::types::{
	AnyModel, Context, ImageModel, Model, ModelType, ProviderStreamOptions, ProviderStreams, SimpleStreamOptions,
	StopReason, TranscriptContext,
};
use pi_ai::utils::event_stream::AssistantMessageEventStream;
use pi_host::PiResult;
use serde_json::{Value, json};

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

fn image_model_json(provider: &str, id: &str) -> Value {
	json!({
		"type": "image",
		"id": id,
		"name": id,
		"api": "test-images",
		"provider": provider,
		"baseUrl": "https://example.test/v1",
		"input": ["text"],
		"output": ["image"],
		"cost": { "input": 0, "output": 0, "cacheRead": 0, "cacheWrite": 0 },
	})
}

fn with_type(mut model: Value, model_type: &str) -> Value {
	model["type"] = json!(model_type);
	model
}

fn image_model(provider: &str, id: &str) -> ImageModel {
	serde_json::from_value(image_model_json(provider, id)).unwrap()
}

fn any_model(value: Value) -> AnyModel {
	serde_json::from_value(value).unwrap()
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

fn model_id(model: &AnyModel) -> String {
	serde_json::to_value(model).unwrap()["id"].as_str().unwrap().to_string()
}

mod chat_models_without_a_type {
	use super::*;

	#[test]
	fn work_through_a_handwritten_provider_without_get_all_models() {
		pi_host::testing::run(|_| async {
			let faux = faux_provider(Some(RegisterFauxProviderOptions {
				provider: Some("handwritten".into()),
				..RegisterFauxProviderOptions::default()
			}));
			let faux_models = faux.models.clone();
			let listed = faux_models.clone();
			let handwritten = Provider {
				id: "handwritten".into(),
				name: "Handwritten".into(),
				base_url: None,
				headers: None,
				auth: faux.provider.auth.clone(),
				get_models: Rc::new(move || listed.clone()),
				get_all_models: None,
				refresh_models: None,
				filter_models: None,
				filter_all_models: None,
				stream: faux.provider.stream.clone(),
				stream_simple: faux.provider.stream_simple.clone(),
				fetch_deferred: None,
				cancel_deferred: None,
				generate_images: None,
				classify: None,
			};
			let models = create_models(None);
			models.set_provider(Rc::new(handwritten));

			let model = models
				.get_model("handwritten", &faux_models[0].id)
				.expect("model is defined");
			assert_eq!(model.r#type, None);
			assert_eq!(get_model_type(&model), "chat");
			let as_any = AnyModel::Chat(model.clone());
			assert!(has_api(&as_any, &model.api).is_some());
			let typed = with_type(serde_json::to_value(&model).unwrap(), "chat");
			assert!(models_are_equal(Some(&model), Some(&any_model(typed))));
			assert!(!models_are_equal(
				Some(&model),
				Some(&image_model(&model.provider, &model.id))
			));
			let expected: Vec<AnyModel> = faux_models.iter().cloned().map(AnyModel::Chat).collect();
			assert_eq!(
				models.get_models_of_type(ModelType::Chat, Some("handwritten")),
				expected
			);
			assert_eq!(models.get_all_models(Some("handwritten")), expected);
			assert_eq!(models.get_models_of_type(ModelType::Image, Some("handwritten")), vec![]);

			faux.set_responses(vec![faux_assistant_message("hi", None).into()]);
			let context: Context = serde_json::from_value(json!({
				"messages": [{ "role": "user", "content": "hi", "timestamp": 0 }],
			}))
			.unwrap();
			let result = models
				.complete(&model, &context, None)
				.await
				.expect("complete resolves");
			assert_eq!(result.stop_reason, StopReason::Stop);
		});
	}

	#[test]
	fn narrow_mixed_lists_with_is_model_type() {
		let mixed = [
			any_model(chat_model_json("p", "c")),
			any_model(with_type(chat_model_json("p", "typed"), "chat")),
			any_model(image_model_json("p", "i")),
		];
		let ids = |model_type| {
			mixed
				.iter()
				.filter(|model| is_model_type(*model, model_type))
				.map(model_id)
				.collect::<Vec<_>>()
		};
		assert_eq!(ids(ModelType::Chat), ["c", "typed"]);
		assert_eq!(ids(ModelType::Image), ["i"]);
		assert!(ids(ModelType::Classifier).is_empty());
	}
}

mod built_in_catalog_getters {
	use super::*;

	#[test]
	fn return_model_shapes_that_can_be_reassigned_within_one_api() {
		let mut model = get_builtin_model("openai", "gpt-4o-mini").unwrap();
		assert_eq!(model.id, "gpt-4o-mini");
		model = get_builtin_model("openai", "gpt-4o").unwrap();
		let mut compat = get_compat_model("openai", "gpt-4o-mini").unwrap();
		assert_eq!(compat.id, "gpt-4o-mini");
		compat = get_compat_model("openai", "gpt-4o").unwrap();

		assert_eq!(model.id, "gpt-4o");
		assert_eq!(compat.id, "gpt-4o");
	}
}

mod stored_and_fetched_models_of_unknown_types {
	use super::*;

	#[test]
	fn are_dropped_instead_of_failing_the_refresh() {
		pi_host::testing::run(|_| async {
			let models_store = Rc::new(InMemoryModelsStore::new());
			let stored: ModelsStoreEntry = serde_json::from_value(json!({
				"models": [
					chat_model_json("dyn", "stored-chat"),
					image_model_json("dyn", "stored-image"),
					with_type(chat_model_json("dyn", "future-embedding"), "embedding"),
					with_type(image_model_json("dyn", "future-video"), "video"),
				],
			}))
			.unwrap();
			models_store.write("dyn", stored, None).await.unwrap();

			let fetched: Rc<RefCell<Vec<AnyModel>>> = Rc::default();
			let next = fetched.clone();
			let models = create_models(Some(CreateModelsOptions {
				models_store: Some(models_store.clone()),
				..CreateModelsOptions::default()
			}));
			models.set_provider(Rc::new(
				create_provider(CreateProviderOptions {
					id: "dyn".into(),
					auth: no_auth(),
					models: vec![],
					fetch_models: Some(Rc::new(move |_| {
						let models = next.borrow().clone();
						Box::pin(async move { Ok(models) })
					})),
					api: Some(ProviderApi::Single(Rc::new(EmptyStreams))),
					..CreateProviderOptions::default()
				})
				.unwrap(),
			));
			let refresh = |allow_network| ModelsRefreshOptions {
				providers: Some(vec!["dyn".into()]),
				allow_network,
				..ModelsRefreshOptions::default()
			};
			let ids = |models: Vec<AnyModel>| models.iter().map(model_id).collect::<Vec<_>>();

			let restored = models.refresh(Some(refresh(Some(false)))).await.unwrap();
			assert!(restored.errors.is_empty());
			assert_eq!(ids(models.get_all_models(Some("dyn"))), ["stored-chat", "stored-image"]);

			*fetched.borrow_mut() = vec![
				any_model(chat_model_json("dyn", "fetched-chat")),
				any_model(with_type(image_model_json("dyn", "fetched-video"), "video")),
			];
			let refreshed = models.refresh(Some(refresh(None))).await.unwrap();
			assert!(refreshed.errors.is_empty());
			assert_eq!(ids(models.get_all_models(Some("dyn"))), ["fetched-chat"]);
			let stored = models_store.read("dyn", None).await.unwrap().unwrap();
			assert_eq!(ids(stored.models), ["fetched-chat"]);
		});
	}
}
