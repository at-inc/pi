//! Port of `packages/ai/src/providers/deepseek.ts`.
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
use crate::providers::deepseek_models::DEEPSEEK_MODELS;
use crate::types::AnyModel;

pub fn deepseek_provider() -> Provider {
	create_provider(CreateProviderOptions {
		id: "deepseek".into(),
		name: Some("DeepSeek".into()),
		base_url: Some("https://api.deepseek.com".into()),
		auth: ProviderAuth {
			api_key: Some(env_api_key_auth("DeepSeek API key", &["DEEPSEEK_API_KEY"])),
			oauth: None,
		},
		models: DEEPSEEK_MODELS.values().cloned().map(AnyModel::Chat).collect(),
		api: Some(ProviderApi::Single(openai_completions_api())),
		..CreateProviderOptions::default()
	})
	.expect("a provider with an api implementation always builds")
}
