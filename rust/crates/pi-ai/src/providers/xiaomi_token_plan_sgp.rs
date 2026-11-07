//! Port of `packages/ai/src/providers/xiaomi-token-plan-sgp.ts`.
//! PORT-STATUS: ported
//!
//! Differences from the TS:
//! - `Provider<TApi>`'s type parameter only narrows the API type, so the factory returns a plain `Provider`.
//! - `createProvider()` throws only without an implementation, which this factory always passes, so the factory
//!   returns the provider instead of a `PiResult`.

use crate::api::openai_completions_lazy::openai_completions_api;
use crate::auth::helpers::env_api_key_auth;
use crate::auth::types::ProviderAuth;
use crate::models::{CreateProviderOptions, Provider, ProviderApi, create_provider};
use crate::providers::xiaomi_token_plan_sgp_models::XIAOMI_TOKEN_PLAN_SGP_MODELS;
use crate::types::AnyModel;

pub fn xiaomi_token_plan_sgp_provider() -> Provider {
	create_provider(CreateProviderOptions {
		id: "xiaomi-token-plan-sgp".into(),
		name: Some("Xiaomi Token Plan SGP".into()),
		base_url: Some("https://token-plan-sgp.xiaomimimo.com/v1".into()),
		auth: ProviderAuth {
			api_key: Some(env_api_key_auth(
				"Xiaomi Token Plan SGP API key",
				&["XIAOMI_TOKEN_PLAN_SGP_API_KEY"],
			)),
			oauth: None,
		},
		models: XIAOMI_TOKEN_PLAN_SGP_MODELS
			.values()
			.cloned()
			.map(AnyModel::Chat)
			.collect(),
		api: Some(ProviderApi::Single(openai_completions_api())),
		..CreateProviderOptions::default()
	})
	.expect("a provider with an api implementation always builds")
}
