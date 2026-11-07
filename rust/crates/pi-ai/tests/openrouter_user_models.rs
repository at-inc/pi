//! Port of `packages/ai/test/openrouter-user-models.test.ts`.
//!
//! `toBe` identity checks compare by value: the Rust function returns a copy of the catalog.

use pi_ai::auth::types::{ApiKeyCredential, Credential};
use pi_ai::models::RefreshModelsContext;
use pi_ai::providers::openrouter::personalize_open_router_catalog;
use pi_ai::types::{AnyModel, BaseModel, ClassifierModel, ImageModel, Model, ModelCost, ModelInputModality};
use pi_host::AbortController;

fn zero_cost() -> ModelCost {
	ModelCost {
		input: 0.0,
		output: 0.0,
		cache_read: 0.0,
		cache_write: 0.0,
		..ModelCost::default()
	}
}

fn chat_model(id: &str) -> Model {
	Model {
		base: BaseModel {
			id: id.into(),
			name: id.into(),
			api: "openai-completions".into(),
			provider: "openrouter".into(),
			base_url: "https://openrouter.ai/api/v1".into(),
			input: vec![ModelInputModality::Text],
			cost: ModelCost {
				input: 1.0,
				output: 2.0,
				cache_read: 0.1,
				cache_write: 0.0,
				..ModelCost::default()
			},
			..BaseModel::default()
		},
		reasoning: false,
		context_window: 200_000.0,
		max_tokens: 50_000.0,
		..Model::default()
	}
}

fn image_model() -> ImageModel {
	ImageModel {
		base: BaseModel {
			id: "image".into(),
			name: "image".into(),
			api: "openrouter-images".into(),
			provider: "openrouter".into(),
			base_url: "https://openrouter.ai/api/v1".into(),
			input: vec![ModelInputModality::Text],
			cost: zero_cost(),
			..BaseModel::default()
		},
		output: vec![ModelInputModality::Image],
		..ImageModel::default()
	}
}

fn classifier_model() -> ClassifierModel {
	ClassifierModel {
		base: BaseModel {
			id: "classifier".into(),
			name: "classifier".into(),
			api: "typesafe-system-one".into(),
			provider: "openrouter".into(),
			base_url: "https://openrouter.ai/api/v1".into(),
			input: vec![ModelInputModality::Text],
			cost: zero_cost(),
			..BaseModel::default()
		},
		context_window: 64_000.0,
		..ClassifierModel::default()
	}
}

fn refresh_context() -> RefreshModelsContext {
	RefreshModelsContext {
		credential: Some(Credential::ApiKey(ApiKeyCredential {
			key: Some("sk-or-test".into()),
			env: None,
		})),
		stored: None,
		publish: std::rc::Rc::new(|_| Box::pin(async { Ok(true) })),
		allow_network: true,
		force: None,
		signal: AbortController::new().signal(),
		base_url: None,
	}
}

fn chat(model: &AnyModel) -> &Model {
	match model {
		AnyModel::Chat(model) => model,
		other => panic!("expected a chat model, got {other:?}"),
	}
}

mod personalize_open_router_catalog {
	use super::*;

	#[test]
	fn keeps_only_the_keys_chat_models_with_the_keys_limits_and_prices() {
		pi_host::testing::run(|host| async move {
			// "no-limits": a missing limit keeps the catalog value, so generator fixes for missing OpenRouter data survive.
			// "router": router models report -1, meaning the price depends on the model they pick.
			host.respond_with(
				200,
				&[("content-type", "application/json")],
				r#"{"data":[{"id":"listed","context_length":1000000,"top_provider":{"context_length":262144,"max_completion_tokens":32768},"pricing":{"prompt":"0.000003","completion":"0.000015"}},{"id":"no-limits","context_length":0,"top_provider":{"context_length":null,"max_completion_tokens":null}},{"id":"router","pricing":{"prompt":"-1","completion":"-1"}}]}"#,
			);
			let catalog = vec![
				AnyModel::Chat(chat_model("listed")),
				AnyModel::Chat(chat_model("no-limits")),
				AnyModel::Chat(chat_model("router")),
				AnyModel::Chat(chat_model("blocked")),
				AnyModel::Image(image_model()),
				AnyModel::Classifier(classifier_model()),
			];

			let result = personalize_open_router_catalog(&catalog, &refresh_context())
				.await
				.unwrap();

			let ids: Vec<&str> = result
				.iter()
				.map(|model| match model {
					AnyModel::Chat(model) => model.id.as_str(),
					AnyModel::Image(model) => model.id.as_str(),
					AnyModel::Classifier(model) => model.id.as_str(),
					AnyModel::Other(_) => "other",
				})
				.collect();
			assert_eq!(ids, ["listed", "no-limits", "router", "image", "classifier"]);
			assert_eq!(chat(&result[0]).context_window, 262_144.0);
			assert_eq!(chat(&result[0]).max_tokens, 32_768.0);
			assert_eq!(
				chat(&result[0]).cost,
				ModelCost {
					input: 3.0,
					output: 15.0,
					cache_read: 0.0,
					cache_write: 0.0,
					..ModelCost::default()
				}
			);
			assert_eq!(chat(&result[1]).context_window, 200_000.0);
			assert_eq!(chat(&result[1]).max_tokens, 50_000.0);
			assert_eq!(chat(&result[1]).cost, chat_model("x").cost);
			assert_eq!(chat(&result[2]).cost, zero_cost());
			// /models/user lists text models only, so image and classifier models are not filtered.
			assert_eq!(result[3], AnyModel::Image(image_model()));
			assert_eq!(result[4], AnyModel::Classifier(classifier_model()));
		});
	}

	#[test]
	fn returns_the_catalog_unchanged_when_the_endpoint_rejects_the_key_or_is_missing() {
		for status in [401, 403, 404] {
			pi_host::testing::run(|host| async move {
				// For example a proxy configured in models.json that rejects the key or has no /models/user:
				// the user keeps the full list instead of a failing refresh.
				host.respond_with(status, &[], "");
				let catalog = vec![AnyModel::Chat(chat_model("a"))];

				assert_eq!(
					personalize_open_router_catalog(&catalog, &refresh_context())
						.await
						.unwrap(),
					catalog,
					"status {status}"
				);
			});
		}
	}
}
