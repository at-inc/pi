//! Port of `packages/ai/src/providers/zai-coding-cn.ts`.
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
use crate::providers::zai_coding_cn_models::ZAI_CODING_CN_MODELS;
use crate::types::AnyModel;

pub fn zai_coding_cn_provider() -> Provider {
	create_provider(CreateProviderOptions {
		id: "zai-coding-cn".into(),
		name: Some("Z.AI Coding CN".into()),
		base_url: Some("https://open.bigmodel.cn/api/coding/paas/v4".into()),
		auth: ProviderAuth {
			api_key: Some(env_api_key_auth("Z.AI Coding CN API key", &["ZAI_CODING_CN_API_KEY"])),
			oauth: None,
		},
		models: ZAI_CODING_CN_MODELS.values().cloned().map(AnyModel::Chat).collect(),
		api: Some(ProviderApi::Single(openai_completions_api())),
		..CreateProviderOptions::default()
	})
	.expect("a provider with an api implementation always builds")
}
