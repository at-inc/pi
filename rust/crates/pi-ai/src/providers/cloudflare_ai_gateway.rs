//! Port of `packages/ai/src/providers/cloudflare-ai-gateway.ts`.
//! PORT-STATUS: ported
//!
//! Differences from the TS:
//! - `Provider<TApi>`'s type parameter only narrows the API type, so the factory returns a plain `Provider`.
//! - `createProvider()` throws only without an implementation, which this factory always passes, so the factory
//!   returns the provider instead of a `PiResult`.

use indexmap::IndexMap;

use crate::api::anthropic_messages_lazy::anthropic_messages_api;
use crate::api::openai_completions_lazy::openai_completions_api;
use crate::api::openai_responses_lazy::openai_responses_api;
use crate::auth::types::ProviderAuth;
use crate::models::{CreateProviderOptions, Provider, ProviderApi, create_provider};
use crate::providers::cloudflare_ai_gateway_models::CLOUDFLARE_AI_GATEWAY_MODELS;
use crate::providers::cloudflare_auth::cloudflare_ai_gateway_auth;
use crate::providers::cloudflare_stream::cloudflare_streams;
use crate::types::AnyModel;

pub fn cloudflare_ai_gateway_provider() -> Provider {
	// The api map is pinned to all three APIs: models.dev's gateway catalog drops and
	// restores `workers-ai/*` (openai-completions) entries over time, and inference from
	// `models` alone would otherwise reject the openai-completions entry whenever the
	// generated catalog happens to contain none.
	create_provider(CreateProviderOptions {
		id: "cloudflare-ai-gateway".into(),
		name: Some("Cloudflare AI Gateway".into()),
		auth: ProviderAuth {
			api_key: Some(cloudflare_ai_gateway_auth()),
			oauth: None,
		},
		models: CLOUDFLARE_AI_GATEWAY_MODELS
			.values()
			.cloned()
			.map(AnyModel::Chat)
			.collect(),
		api: Some(ProviderApi::ByApi(IndexMap::from([
			(
				"anthropic-messages".to_string(),
				cloudflare_streams(anthropic_messages_api()),
			),
			(
				"openai-completions".to_string(),
				cloudflare_streams(openai_completions_api()),
			),
			(
				"openai-responses".to_string(),
				cloudflare_streams(openai_responses_api()),
			),
		]))),
		..CreateProviderOptions::default()
	})
	.expect("a provider with an api implementation always builds")
}
