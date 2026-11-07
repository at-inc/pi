//! Port of `packages/ai/src/providers/cloudflare-workers-ai.ts`.
//! PORT-STATUS: ported
//!
//! Differences from the TS:
//! - `Provider<TApi>`'s type parameter only narrows the API type, so the factory returns a plain `Provider`.
//! - `createProvider()` throws only without an implementation, which this factory always passes, so the factory
//!   returns the provider instead of a `PiResult`.

use indexmap::IndexMap;

use crate::api::cloudflare_workers_ai_system_one_lazy::cloudflare_workers_ai_system_one_api;
use crate::api::openai_completions_lazy::openai_completions_api;
use crate::auth::types::ProviderAuth;
use crate::models::{CreateProviderOptions, Provider, ProviderApi, create_provider};
use crate::providers::cloudflare_auth::cloudflare_workers_ai_auth;
use crate::providers::cloudflare_stream::{cloudflare_classifier, cloudflare_streams};
use crate::providers::cloudflare_workers_ai_models::{
	CLOUDFLARE_WORKERS_AI_CLASSIFIER_MODELS, CLOUDFLARE_WORKERS_AI_MODELS,
};
use crate::types::AnyModel;

pub fn cloudflare_workers_ai_provider() -> Provider {
	create_provider(CreateProviderOptions {
		id: "cloudflare-workers-ai".into(),
		name: Some("Cloudflare Workers AI".into()),
		auth: ProviderAuth {
			api_key: Some(cloudflare_workers_ai_auth()),
			oauth: None,
		},
		models: CLOUDFLARE_WORKERS_AI_MODELS
			.values()
			.cloned()
			.map(AnyModel::Chat)
			.chain(
				CLOUDFLARE_WORKERS_AI_CLASSIFIER_MODELS
					.values()
					.cloned()
					.map(AnyModel::Classifier),
			)
			.collect(),
		api: Some(ProviderApi::Single(cloudflare_streams(openai_completions_api()))),
		classifiers: Some(IndexMap::from([(
			"cloudflare-workers-ai-system-one".to_string(),
			cloudflare_classifier(cloudflare_workers_ai_system_one_api()),
		)])),
		..CreateProviderOptions::default()
	})
	.expect("a provider with an api implementation always builds")
}
