//! Port of `packages/ai/src/providers/mistral.ts`.
//! PORT-STATUS: ported
//!
//! Differences from the TS:
//! - `Provider<TApi>`'s type parameter only narrows the API type, so the factory returns a plain `Provider`.
//! - `createProvider()` throws only without an implementation, which this factory always passes, so the factory
//!   returns the provider instead of a `PiResult`.

use crate::api::mistral_conversations_lazy::mistral_conversations_api;
use crate::auth::helpers::env_api_key_auth;
use crate::auth::types::ProviderAuth;
use crate::models::{CreateProviderOptions, Provider, ProviderApi, create_provider};
use crate::providers::mistral_models::MISTRAL_MODELS;
use crate::types::AnyModel;

pub fn mistral_provider() -> Provider {
	create_provider(CreateProviderOptions {
		id: "mistral".into(),
		name: Some("Mistral".into()),
		base_url: Some("https://api.mistral.ai".into()),
		auth: ProviderAuth {
			api_key: Some(env_api_key_auth("Mistral API key", &["MISTRAL_API_KEY"])),
			oauth: None,
		},
		models: MISTRAL_MODELS.values().cloned().map(AnyModel::Chat).collect(),
		api: Some(ProviderApi::Single(mistral_conversations_api())),
		..CreateProviderOptions::default()
	})
	.expect("a provider with an api implementation always builds")
}
