//! Port of `packages/ai/src/providers/openai-codex-images.ts`.
//! PORT-STATUS: ported
//!
//! Differences from the TS:
//! - `Provider<TApi>`'s type parameter only narrows the API type, so the factory returns a plain `Provider`.
//! - `createProvider()` throws only without an implementation, which this factory always passes, so the factory
//!   returns the provider instead of a `PiResult`.

use indexmap::IndexMap;

use crate::api::openai_codex_images_lazy::openai_codex_images_api;
use crate::models::{CreateProviderOptions, Provider, create_provider};
use crate::providers::openai_codex::openai_codex_provider;
use crate::providers::openai_codex_models::OPENAI_CODEX_IMAGE_MODELS;
use crate::types::AnyModel;

pub fn openai_codex_images_provider() -> Provider {
	create_provider(CreateProviderOptions {
		id: "openai-codex".into(),
		name: Some("ChatGPT".into()),
		auth: openai_codex_provider().auth,
		models: OPENAI_CODEX_IMAGE_MODELS
			.values()
			.cloned()
			.map(AnyModel::Image)
			.collect(),
		images: Some(IndexMap::from([(
			"openai-codex-images".to_string(),
			openai_codex_images_api(),
		)])),
		..CreateProviderOptions::default()
	})
	.expect("a provider with an api implementation always builds")
}
