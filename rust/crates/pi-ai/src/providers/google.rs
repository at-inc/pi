//! Port of `packages/ai/src/providers/google.ts`.
//! PORT-STATUS: ported
//!
//! Differences from the TS:
//! - `Provider<TApi>`'s type parameter only narrows the API type, so the factory returns a plain `Provider`.
//! - `createProvider()` throws only without an implementation, which this factory always passes, so the factory
//!   returns the provider instead of a `PiResult`.

use crate::api::google_generative_ai_lazy::google_generative_ai_api;
use crate::auth::helpers::env_api_key_auth;
use crate::auth::types::ProviderAuth;
use crate::models::{CreateProviderOptions, Provider, ProviderApi, create_provider};
use crate::providers::google_models::GOOGLE_MODELS;
use crate::types::AnyModel;

pub fn google_provider() -> Provider {
	create_provider(CreateProviderOptions {
		id: "google".into(),
		name: Some("Google".into()),
		base_url: Some("https://generativelanguage.googleapis.com/v1beta".into()),
		auth: ProviderAuth {
			api_key: Some(env_api_key_auth("Gemini API key", &["GEMINI_API_KEY"])),
			oauth: None,
		},
		models: GOOGLE_MODELS.values().cloned().map(AnyModel::Chat).collect(),
		api: Some(ProviderApi::Single(google_generative_ai_api())),
		..CreateProviderOptions::default()
	})
	.expect("a provider with an api implementation always builds")
}
