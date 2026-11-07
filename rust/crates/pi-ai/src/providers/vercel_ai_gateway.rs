//! Port of `packages/ai/src/providers/vercel-ai-gateway.ts`.
//! PORT-STATUS: ported
//!
//! Differences from the TS:
//! - `Provider<TApi>`'s type parameter only narrows the API type, so the factory returns a plain `Provider`.
//! - `createProvider()` throws only without an implementation, which this factory always passes, so the factory
//!   returns the provider instead of a `PiResult`.

use indexmap::IndexMap;

use crate::api::anthropic_messages_lazy::anthropic_messages_api;
use crate::api::typesafe_system_one_lazy::typesafe_system_one_api;
use crate::auth::helpers::env_api_key_auth;
use crate::auth::types::ProviderAuth;
use crate::models::{CreateProviderOptions, Provider, ProviderApi, create_provider};
use crate::providers::vercel_ai_gateway_models::{VERCEL_AI_GATEWAY_CLASSIFIER_MODELS, VERCEL_AI_GATEWAY_MODELS};
use crate::types::AnyModel;

pub fn vercel_ai_gateway_provider() -> Provider {
	create_provider(CreateProviderOptions {
		id: "vercel-ai-gateway".into(),
		name: Some("Vercel AI Gateway".into()),
		base_url: Some("https://ai-gateway.vercel.sh".into()),
		auth: ProviderAuth {
			api_key: Some(env_api_key_auth("Vercel AI Gateway API key", &["AI_GATEWAY_API_KEY"])),
			oauth: None,
		},
		models: VERCEL_AI_GATEWAY_MODELS
			.values()
			.cloned()
			.map(AnyModel::Chat)
			.chain(
				VERCEL_AI_GATEWAY_CLASSIFIER_MODELS
					.values()
					.cloned()
					.map(AnyModel::Classifier),
			)
			.collect(),
		api: Some(ProviderApi::Single(anthropic_messages_api())),
		// AI Gateway serves TypeSafe's System One protocol at /typesafe/v1/systemone.
		classifiers: Some(IndexMap::from([(
			"typesafe-system-one".to_string(),
			typesafe_system_one_api(),
		)])),
		..CreateProviderOptions::default()
	})
	.expect("a provider with an api implementation always builds")
}
