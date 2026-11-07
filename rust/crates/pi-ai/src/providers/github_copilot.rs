//! Port of `packages/ai/src/providers/github-copilot.ts`.
//! PORT-STATUS: ported
//!
//! Differences from the TS:
//! - `Provider<TApi>`'s type parameter only narrows the API type, so the factory returns a plain `Provider`.
//! - `createProvider()` throws only without an implementation, which this factory always passes, so the factory
//!   returns the provider instead of a `PiResult`.

use std::rc::Rc;

use indexmap::{IndexMap, IndexSet};
use serde_json::Value;

use crate::api::anthropic_messages_lazy::anthropic_messages_api;
use crate::api::openai_completions_lazy::openai_completions_api;
use crate::api::openai_responses_lazy::openai_responses_api;
use crate::auth::helpers::{LazyOAuthInput, env_api_key_auth, lazy_oauth};
use crate::auth::oauth::load::load_github_copilot_oauth;
use crate::auth::types::{Credential, ProviderAuth};
use crate::models::{CreateProviderOptions, Provider, ProviderApi, create_provider};
use crate::providers::github_copilot_models::GITHUB_COPILOT_MODELS;
use crate::types::{AnyModel, Model};

pub fn github_copilot_provider() -> Provider {
	create_provider(CreateProviderOptions {
		id: "github-copilot".into(),
		name: Some("GitHub Copilot".into()),
		base_url: Some("https://api.individual.githubcopilot.com".into()),
		auth: ProviderAuth {
			api_key: Some(env_api_key_auth("GitHub Copilot token", &["COPILOT_GITHUB_TOKEN"])),
			oauth: Some(lazy_oauth(LazyOAuthInput {
				name: "GitHub Copilot".into(),
				is_subscription: Some(true),
				login_label: None,
				load: Rc::new(|| Box::pin(load_github_copilot_oauth())),
			})),
		},
		models: GITHUB_COPILOT_MODELS.values().cloned().map(AnyModel::Chat).collect(),
		filter_models: Some(Rc::new(|models: &[Model], credential: Option<&Credential>| {
			let Some(Credential::OAuth(credential)) = credential else {
				return models.to_vec();
			};
			let Some(Value::Array(available_model_ids)) = credential.extra.get("availableModelIds") else {
				return models.to_vec();
			};
			let Some(available) = available_model_ids
				.iter()
				.map(Value::as_str)
				.collect::<Option<IndexSet<&str>>>()
			else {
				return models.to_vec();
			};
			models
				.iter()
				.filter(|model| available.contains(model.id.as_str()))
				.cloned()
				.collect()
		})),
		api: Some(ProviderApi::ByApi(IndexMap::from([
			("anthropic-messages".to_string(), anthropic_messages_api()),
			("openai-completions".to_string(), openai_completions_api()),
			("openai-responses".to_string(), openai_responses_api()),
		]))),
		..CreateProviderOptions::default()
	})
	.expect("a provider with an api implementation always builds")
}
