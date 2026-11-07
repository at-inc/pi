//! Port of `packages/ai/src/providers/xai.ts`.
//! PORT-STATUS: reviewed
//!
//! Differences from the TS:
//! - `Provider<TApi>`'s type parameter only narrows the API type, so the factory returns a plain `Provider`.
//! - `createProvider()` throws only without an implementation, which this factory always passes, so the factory
//!   returns the provider instead of a `PiResult`.

use std::rc::Rc;

use crate::api::openai_responses_lazy::openai_responses_api;
use crate::auth::helpers::{LazyOAuthInput, env_api_key_auth, lazy_oauth};
use crate::auth::oauth::load::load_xai_oauth;
use crate::auth::types::ProviderAuth;
use crate::models::{CreateProviderOptions, Provider, ProviderApi, create_provider};
use crate::providers::xai_models::XAI_MODELS;
use crate::types::AnyModel;

pub fn xai_provider() -> Provider {
	create_provider(CreateProviderOptions {
		id: "xai".into(),
		name: Some("xAI".into()),
		base_url: Some("https://api.x.ai/v1".into()),
		auth: ProviderAuth {
			api_key: Some(env_api_key_auth("xAI API key", &["XAI_API_KEY"])),
			oauth: Some(lazy_oauth(LazyOAuthInput {
				name: "xAI (Grok/X subscription)".into(),
				is_subscription: Some(true),
				login_label: Some("Sign in with SuperGrok or X Premium".into()),
				load: Rc::new(|| Box::pin(load_xai_oauth())),
			})),
		},
		models: XAI_MODELS.values().cloned().map(AnyModel::Chat).collect(),
		api: Some(ProviderApi::Single(openai_responses_api())),
		..CreateProviderOptions::default()
	})
	.expect("a provider with an api implementation always builds")
}
