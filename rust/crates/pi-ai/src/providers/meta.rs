//! Port of `packages/ai/src/providers/meta.ts`.
//! PORT-STATUS: reviewed
//!
//! Differences from the TS:
//! - `Provider<TApi>`'s type parameter only narrows the API type, so the factory returns a plain `Provider`.
//! - `createProvider()` throws only without an implementation, which this factory always passes, so the factory
//!   returns the provider instead of a `PiResult`.

use std::rc::Rc;

use crate::api::openai_responses_lazy::openai_responses_api;
use crate::auth::helpers::{LazyOAuthInput, env_api_key_auth, lazy_oauth};
use crate::auth::oauth::load::load_meta_oauth;
use crate::auth::types::ProviderAuth;
use crate::models::{CreateProviderOptions, Provider, ProviderApi, create_provider};
use crate::providers::meta_models::META_MODELS;
use crate::types::AnyModel;

pub fn meta_provider() -> Provider {
	create_provider(CreateProviderOptions {
		id: "meta".into(),
		name: Some("Meta".into()),
		base_url: Some("https://api.meta.ai/v1".into()),
		auth: ProviderAuth {
			api_key: Some(env_api_key_auth("Meta Model API key", &["META_API_KEY"])),
			oauth: Some(lazy_oauth(LazyOAuthInput {
				name: "Meta (Muse subscription)".into(),
				is_subscription: Some(true),
				login_label: Some("Sign in with Meta".into()),
				load: Rc::new(|| Box::pin(load_meta_oauth())),
			})),
		},
		models: META_MODELS.values().cloned().map(AnyModel::Chat).collect(),
		api: Some(ProviderApi::Single(openai_responses_api())),
		..CreateProviderOptions::default()
	})
	.expect("a provider with an api implementation always builds")
}
