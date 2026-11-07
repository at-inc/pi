//! Port of `packages/ai/src/providers/fireworks.ts`.
//! PORT-STATUS: reviewed
//!
//! Differences from the TS:
//! - `Provider<TApi>`'s type parameter only narrows the API type, so the factory returns a plain `Provider`.
//! - `createProvider()` throws only without an implementation, which this factory always passes, so the factory
//!   returns the provider instead of a `PiResult`.

use indexmap::IndexMap;

use crate::api::anthropic_messages_lazy::anthropic_messages_api;
use crate::api::openai_completions_lazy::openai_completions_api;
use crate::auth::helpers::env_api_key_auth;
use crate::auth::types::ProviderAuth;
use crate::models::{CreateProviderOptions, Provider, ProviderApi, create_provider};
use crate::providers::fireworks_models::FIREWORKS_MODELS;
use crate::types::AnyModel;

pub fn fireworks_provider() -> Provider {
	create_provider(CreateProviderOptions {
		id: "fireworks".into(),
		name: Some("Fireworks".into()),
		base_url: Some("https://api.fireworks.ai/inference".into()),
		auth: ProviderAuth {
			api_key: Some(env_api_key_auth("Fireworks API key", &["FIREWORKS_API_KEY"])),
			oauth: None,
		},
		models: FIREWORKS_MODELS.values().cloned().map(AnyModel::Chat).collect(),
		api: Some(ProviderApi::ByApi(IndexMap::from([
			("anthropic-messages".to_string(), anthropic_messages_api()),
			("openai-completions".to_string(), openai_completions_api()),
		]))),
		..CreateProviderOptions::default()
	})
	.expect("a provider with an api implementation always builds")
}
