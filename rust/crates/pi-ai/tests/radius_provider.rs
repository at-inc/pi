//! Port of `packages/ai/test/radius-provider.test.ts`.

use std::rc::Rc;

use pi_ai::auth::credential_store::InMemoryCredentialStore;
use pi_ai::auth::types::{ApiKeyCredential, Credential, CredentialStore};
use pi_ai::models::{CreateModelsOptions, ModelsRefreshOptions, MutableModels, create_models};
use pi_ai::models_store::{InMemoryModelsStore, ModelsStore, ModelsStoreEntry};
use pi_ai::providers::radius::{RadiusProviderOptions, radius_provider};
use pi_ai::providers::radius_config::{RadiusGatewayConfig, RadiusGatewayModel, get_radius_models_from_config};
use pi_ai::types::{AnyModel, ModelCost, ModelInputModality};

fn cost(input: f64, output: f64, cache_read: f64) -> ModelCost {
	ModelCost {
		input,
		output,
		cache_read,
		cache_write: 0.0,
		..ModelCost::default()
	}
}

fn radius_config() -> RadiusGatewayConfig {
	RadiusGatewayConfig {
		base_url: "https://radius.example/v1".into(),
		models: vec![
			RadiusGatewayModel {
				id: "balanced".into(),
				name: "Fresh Balanced".into(),
				reasoning: true,
				thinking_level_map: None,
				input: vec![ModelInputModality::Text],
				cost: cost(1.0, 2.0, 0.1),
				context_window: 424242.0,
				max_tokens: 32000.0,
				extra: Default::default(),
			},
			RadiusGatewayModel {
				id: "organization-only".into(),
				name: "Organization Only".into(),
				reasoning: false,
				thinking_level_map: None,
				input: vec![ModelInputModality::Text],
				cost: cost(0.0, 0.0, 0.0),
				context_window: 128000.0,
				max_tokens: 16000.0,
				extra: Default::default(),
			},
		],
	}
}

fn model_ids(models: &dyn MutableModels) -> Vec<String> {
	models
		.get_models(Some("radius"))
		.into_iter()
		.map(|model| model.id.clone())
		.collect()
}

async fn models_with_radius_key() -> Rc<dyn MutableModels> {
	let credentials = Rc::new(InMemoryCredentialStore::new());
	credentials
		.modify(
			"radius",
			Box::new(|_| {
				Box::pin(async {
					Ok(Some(Credential::ApiKey(ApiKeyCredential {
						key: Some("radius-key".into()),
						env: None,
						extra: Default::default(),
					})))
				})
			}),
			None,
		)
		.await
		.unwrap();
	let models = create_models(Some(CreateModelsOptions {
		credentials: Some(credentials),
		..CreateModelsOptions::default()
	}));
	models.set_provider(Rc::new(radius_provider(None)));
	models
}

fn refresh_radius(allow_network: Option<bool>) -> ModelsRefreshOptions {
	ModelsRefreshOptions {
		allow_network,
		providers: Some(vec!["radius".into()]),
		..ModelsRefreshOptions::default()
	}
}

mod radius_provider_catalogs {
	use super::*;

	#[test]
	fn ships_a_static_public_catalog_for_the_default_gateway() {
		let provider = radius_provider(None);
		let models = (provider.get_models)();
		assert!(!models.is_empty());
		assert!(
			models
				.iter()
				.any(|model| model.id == "balanced" && model.provider == "radius" && model.api == "pi-messages")
		);
	}

	#[test]
	fn does_not_apply_the_public_radius_catalog_to_custom_gateways() {
		let provider = radius_provider(Some(RadiusProviderOptions {
			id: Some("radius-dev".into()),
			gateway: Some("http://localhost:8788".into()),
			..RadiusProviderOptions::default()
		}));
		assert_eq!((provider.get_models)(), vec![]);
	}

	#[test]
	fn replaces_the_static_public_catalog_with_refreshed_models() {
		pi_host::testing::run(|host| async move {
			let body: &'static str = Box::leak(serde_json::to_string(&radius_config()).unwrap().into_boxed_str());
			host.respond_with(200, &[("content-type", "application/json")], body);
			let models = models_with_radius_key().await;

			let result = models.refresh(Some(refresh_radius(None))).await.unwrap();

			assert!(result.errors.is_empty());
			let balanced = models.get_model("radius", "balanced").unwrap();
			assert_eq!(balanced.name, "Fresh Balanced");
			assert_eq!(balanced.base_url, "https://radius.example/v1");
			assert_eq!(balanced.context_window, 424242.0);
			assert!(models.get_model("radius", "organization-only").is_some());
			// Models disabled by the organization must not reappear from the shipped catalog.
			assert_eq!(model_ids(models.as_ref()), ["balanced", "organization-only"]);
		});
	}

	#[test]
	fn replaces_the_static_public_catalog_with_a_cached_catalog_without_network_access() {
		pi_host::testing::run(|host| async move {
			let store = Rc::new(InMemoryModelsStore::new());
			store
				.write(
					"radius",
					ModelsStoreEntry {
						models: get_radius_models_from_config("radius", &radius_config())
							.into_iter()
							.map(AnyModel::Chat)
							.collect(),
						checked_at: Some(pi_host::now_ms()),
						..ModelsStoreEntry::default()
					},
					None,
				)
				.await
				.unwrap();
			let models = create_models(Some(CreateModelsOptions {
				models_store: Some(store),
				..CreateModelsOptions::default()
			}));
			models.set_provider(Rc::new(radius_provider(None)));

			models.refresh(Some(refresh_radius(Some(false)))).await.unwrap();

			assert_eq!(models.get_model("radius", "balanced").unwrap().name, "Fresh Balanced");
			assert!(models.get_model("radius", "organization-only").is_some());
			assert_eq!(model_ids(models.as_ref()), ["balanced", "organization-only"]);
			assert!(host.requests().is_empty());
		});
	}

	#[test]
	fn exposes_no_models_when_the_organization_disabled_all_of_them() {
		pi_host::testing::run(|host| async move {
			host.respond_with(
				200,
				&[("content-type", "application/json")],
				r#"{"baseUrl":"https://radius.example/v1","models":[]}"#,
			);
			let models = models_with_radius_key().await;

			let result = models.refresh(Some(refresh_radius(None))).await.unwrap();

			assert!(result.errors.is_empty());
			assert_eq!(models.get_models(Some("radius")), vec![]);
		});
	}
}

mod port_regressions {
	use pi_ai::auth::types::OAuthCredential;
	use pi_ai::providers::radius_config::get_radius_credential_config;
	use serde_json::json;

	#[test]
	fn keeps_an_explicit_null_thinking_level_map_in_the_gateway_config() {
		let mut credential = OAuthCredential::new("refresh", "access", 0.0);
		credential.extra.insert(
			"gatewayConfig".into(),
			json!({
				"baseUrl": "https://radius.example/v1",
				"models": [{
					"id": "balanced",
					"name": "Balanced",
					"reasoning": true,
					"thinkingLevelMap": null,
					"input": ["text"],
					"cost": { "input": 1, "output": 2, "cacheRead": 0, "cacheWrite": 0 },
					"contextWindow": 1000,
					"maxTokens": 100
				}]
			}),
		);

		let config = get_radius_credential_config(Some(&credential)).unwrap();
		assert_eq!(config.models[0].thinking_level_map, Some(None));
		assert_eq!(
			serde_json::to_value(&config).unwrap()["models"][0]["thinkingLevelMap"],
			json!(null)
		);
	}
}
