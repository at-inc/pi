//! Port of `packages/ai/src/providers/minimax-cn.ts`.
//! PORT-STATUS: reviewed
//!
//! Differences from the TS:
//! - `Provider<TApi>`'s type parameter only narrows the API type, so the factory returns a plain `Provider`.
//! - `createProvider()` throws only without an implementation, which this factory always passes, so the factory
//!   returns the provider instead of a `PiResult`.

use crate::api::anthropic_messages_lazy::anthropic_messages_api;
use crate::auth::helpers::env_api_key_auth;
use crate::auth::types::ProviderAuth;
use crate::models::{CreateProviderOptions, Provider, ProviderApi, create_provider};
use crate::providers::minimax_cn_models::MINIMAX_CN_MODELS;
use crate::types::AnyModel;

pub fn minimax_cn_provider() -> Provider {
	create_provider(CreateProviderOptions {
		id: "minimax-cn".into(),
		name: Some("MiniMax CN".into()),
		base_url: Some("https://api.minimaxi.com/anthropic".into()),
		auth: ProviderAuth {
			api_key: Some(env_api_key_auth("MiniMax CN API key", &["MINIMAX_CN_API_KEY"])),
			oauth: None,
		},
		models: MINIMAX_CN_MODELS.values().cloned().map(AnyModel::Chat).collect(),
		api: Some(ProviderApi::Single(anthropic_messages_api())),
		..CreateProviderOptions::default()
	})
	.expect("a provider with an api implementation always builds")
}
