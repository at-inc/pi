//! Port of `packages/ai/src/providers/xiaomi-token-plan-cn.ts`.
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
use crate::providers::xiaomi_token_plan_cn_models::XIAOMI_TOKEN_PLAN_CN_MODELS;
use crate::types::AnyModel;

pub fn xiaomi_token_plan_cn_provider() -> Provider {
	create_provider(CreateProviderOptions {
		id: "xiaomi-token-plan-cn".into(),
		name: Some("Xiaomi Token Plan CN".into()),
		base_url: Some("https://token-plan-cn.xiaomimimo.com/v1".into()),
		auth: ProviderAuth {
			api_key: Some(env_api_key_auth(
				"Xiaomi Token Plan CN API key",
				&["XIAOMI_TOKEN_PLAN_CN_API_KEY"],
			)),
			oauth: None,
		},
		models: XIAOMI_TOKEN_PLAN_CN_MODELS
			.values()
			.cloned()
			.map(AnyModel::Chat)
			.collect(),
		api: Some(ProviderApi::Single(openai_completions_api())),
		..CreateProviderOptions::default()
	})
	.expect("a provider with an api implementation always builds")
}
