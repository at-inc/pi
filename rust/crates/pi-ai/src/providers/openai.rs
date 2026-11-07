//! Port of `packages/ai/src/providers/openai.ts`.
//! PORT-STATUS: reviewed
//!
//! Differences from the TS:
//! - `Provider<TApi>`'s type parameter only narrows the API type, so the factory returns a plain `Provider`.
//! - `createProvider()` throws only without an implementation, which this factory always passes, so the factory
//!   returns the provider instead of a `PiResult`.

use std::rc::Rc;

use indexmap::IndexMap;

use crate::api::openai_decisions_lazy::openai_decisions_api;
use crate::api::openai_responses_lazy::openai_responses_api;
use crate::auth::helpers::{LazyOAuthInput, env_api_key_auth, lazy_oauth};
use crate::auth::oauth::load::load_openai_chatgpt_oauth;
use crate::auth::types::{Credential, ProviderAuth};
use crate::models::{CreateProviderOptions, Provider, ProviderApi, ProviderModel, create_provider};
use crate::providers::openai_models::{OPENAI_CLASSIFIER_MODELS, OPENAI_MODELS};
use crate::types::{AnyModel, ModelType};
use crate::utils::model_operations::is_model_type;

pub fn openai_provider() -> Provider {
	create_provider(CreateProviderOptions {
		id: "openai".into(),
		name: Some("OpenAI".into()),
		base_url: Some("https://api.openai.com/v1".into()),
		auth: ProviderAuth {
			api_key: Some(env_api_key_auth("OpenAI API key", &["OPENAI_API_KEY"])),
			oauth: Some(lazy_oauth(LazyOAuthInput {
				name: "OpenAI (ChatGPT subscription)".into(),
				is_subscription: Some(true),
				login_label: Some("Sign in with ChatGPT".into()),
				load: Rc::new(|| Box::pin(load_openai_chatgpt_oauth())),
			})),
		},
		models: OPENAI_MODELS
			.values()
			.cloned()
			.map(AnyModel::Chat)
			.chain(OPENAI_CLASSIFIER_MODELS.values().cloned().map(AnyModel::Classifier))
			.collect(),
		// Sign in with ChatGPT tokens only reach the Responses API; the Decisions API rejects them.
		filter_all_models: Some(Rc::new(|models: &[ProviderModel], credential: Option<&Credential>| {
			if matches!(credential, Some(Credential::OAuth(_))) {
				models
					.iter()
					.filter(|model| !is_model_type(*model, ModelType::Classifier))
					.cloned()
					.collect()
			} else {
				models.to_vec()
			}
		})),
		api: Some(ProviderApi::Single(openai_responses_api())),
		classifiers: Some(IndexMap::from([(
			"openai-decisions".to_string(),
			openai_decisions_api(),
		)])),
		..CreateProviderOptions::default()
	})
	.expect("a provider with an api implementation always builds")
}
