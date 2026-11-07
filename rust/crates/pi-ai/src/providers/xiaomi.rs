//! Port of `packages/ai/src/providers/xiaomi.ts`.
//! PORT-STATUS: reviewed
//!
//! Differences from the TS:
//! - `Provider<TApi>`'s type parameter only narrows the API type, so the factory returns a plain `Provider`.
//! - `createProvider()` throws only without an implementation, which this factory always passes, so the factory
//!   returns the provider instead of a `PiResult`.

use crate::api::openai_completions_lazy::openai_completions_api;
use crate::auth::helpers::env_api_key_auth;
use crate::auth::types::ProviderAuth;
use crate::models::{CreateProviderOptions, Provider, ProviderApi, create_provider};
use crate::providers::xiaomi_models::XIAOMI_MODELS;
use crate::types::AnyModel;

pub fn xiaomi_provider() -> Provider {
	create_provider(CreateProviderOptions {
		id: "xiaomi".into(),
		name: Some("Xiaomi".into()),
		base_url: Some("https://api.xiaomimimo.com/v1".into()),
		auth: ProviderAuth {
			api_key: Some(env_api_key_auth("Xiaomi API key", &["XIAOMI_API_KEY"])),
			oauth: None,
		},
		models: XIAOMI_MODELS.values().cloned().map(AnyModel::Chat).collect(),
		api: Some(ProviderApi::Single(openai_completions_api())),
		..CreateProviderOptions::default()
	})
	.expect("a provider with an api implementation always builds")
}
