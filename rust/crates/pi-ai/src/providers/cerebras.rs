//! Port of `packages/ai/src/providers/cerebras.ts`.
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
use crate::providers::cerebras_models::CEREBRAS_MODELS;
use crate::types::AnyModel;

pub fn cerebras_provider() -> Provider {
	create_provider(CreateProviderOptions {
		id: "cerebras".into(),
		name: Some("Cerebras".into()),
		base_url: Some("https://api.cerebras.ai/v1".into()),
		auth: ProviderAuth {
			api_key: Some(env_api_key_auth("Cerebras API key", &["CEREBRAS_API_KEY"])),
			oauth: None,
		},
		models: CEREBRAS_MODELS.values().cloned().map(AnyModel::Chat).collect(),
		api: Some(ProviderApi::Single(openai_completions_api())),
		..CreateProviderOptions::default()
	})
	.expect("a provider with an api implementation always builds")
}
