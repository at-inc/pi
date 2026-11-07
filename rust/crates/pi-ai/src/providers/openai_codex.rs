//! Port of `packages/ai/src/providers/openai-codex.ts`.
//! PORT-STATUS: ported
//!
//! Differences from the TS:
//! - `Provider<TApi>`'s type parameter only narrows the API type, so the factory returns a plain `Provider`.
//! - `createProvider()` throws only without an implementation, which this factory always passes, so the factory
//!   returns the provider instead of a `PiResult`.

use std::rc::Rc;

use indexmap::IndexMap;

use crate::api::openai_codex_images_lazy::openai_codex_images_api;
use crate::api::openai_codex_responses_lazy::openai_codex_responses_api;
use crate::auth::helpers::{LazyOAuthInput, lazy_oauth};
use crate::auth::oauth::load::load_openai_codex_oauth;
use crate::auth::types::ProviderAuth;
use crate::models::{CreateProviderOptions, Provider, ProviderApi, create_provider};
use crate::providers::openai_codex_models::{OPENAI_CODEX_IMAGE_MODELS, OPENAI_CODEX_MODELS};
use crate::types::AnyModel;

pub const OPENAI_CODEX_BASE_URL: &str = "https://chatgpt.com/backend-api";

pub fn openai_codex_provider() -> Provider {
	create_provider(CreateProviderOptions {
		id: "openai-codex".into(),
		name: Some("OpenAI Codex (legacy)".into()),
		base_url: Some(OPENAI_CODEX_BASE_URL.into()),
		auth: ProviderAuth {
			api_key: None,
			oauth: Some(lazy_oauth(LazyOAuthInput {
				name: "OpenAI (ChatGPT Plus/Pro)".into(),
				is_subscription: Some(true),
				login_label: None,
				load: Rc::new(|| Box::pin(load_openai_codex_oauth())),
			})),
		},
		models: OPENAI_CODEX_MODELS
			.values()
			.cloned()
			.map(AnyModel::Chat)
			.chain(OPENAI_CODEX_IMAGE_MODELS.values().cloned().map(AnyModel::Image))
			.collect(),
		api: Some(ProviderApi::Single(openai_codex_responses_api())),
		images: Some(IndexMap::from([(
			"openai-codex-images".to_string(),
			openai_codex_images_api(),
		)])),
		..CreateProviderOptions::default()
	})
	.expect("a provider with an api implementation always builds")
}
