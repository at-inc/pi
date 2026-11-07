//! Port of `packages/ai/src/providers/anthropic.ts`.
//! PORT-STATUS: ported
//!
//! Differences from the TS:
//! - `Provider<TApi>`'s type parameter only narrows the API type, so the factory returns a plain `Provider`.
//! - `createProvider()` throws only without an implementation, which this factory always passes, so the factory
//!   returns the provider instead of a `PiResult`.

use std::rc::Rc;

use indexmap::IndexMap;

use crate::api::anthropic_messages_lazy::anthropic_messages_api;
use crate::auth::helpers::{LazyOAuthInput, lazy_oauth};
use crate::auth::oauth::load::load_anthropic_oauth;
use crate::auth::types::{
	ApiKeyAuth, ApiKeyAuthInput, ApiKeyCredential, AuthPrompt, AuthResult, ModelAuth, ProviderAuth,
};
use crate::env_api_keys::{
	ANTHROPIC_API_KEY_ENV, ANTHROPIC_AUTH_TOKEN_ENV, ANTHROPIC_FEDERATION_RULE_ID_ENV,
	ANTHROPIC_IDENTITY_TOKEN_FILE_ENV, ANTHROPIC_OAUTH_TOKEN_ENV, ANTHROPIC_ORGANIZATION_ID_ENV,
	ANTHROPIC_SERVICE_ACCOUNT_ID_ENV, ANTHROPIC_WORKSPACE_ID_ENV,
};
use crate::models::{CreateProviderOptions, Provider, ProviderApi, create_provider};
use crate::providers::anthropic_models::ANTHROPIC_MODELS;
use crate::types::{AnyModel, ProviderEnv};

fn anthropic_api_key_auth() -> ApiKeyAuth {
	ApiKeyAuth {
		name: "Anthropic API key".into(),
		login: Some(Rc::new(|interaction| {
			Box::pin(async move {
				interaction.signal.throw_if_aborted()?;
				let key = (interaction.prompt)(AuthPrompt::Secret {
					message: "Enter Anthropic API key".into(),
					placeholder: None,
					signal: None,
				})
				.await?;
				interaction.signal.throw_if_aborted()?;
				Ok(ApiKeyCredential {
					key: Some(key),
					env: None,
				})
			})
		})),
		check: None,
		resolve: Rc::new(
			|ApiKeyAuthInput {
			     ctx,
			     credential,
			     signal,
			 }| {
				Box::pin(async move {
					signal.throw_if_aborted()?;
					if let Some(credential) = &credential
						&& let Some(key) = credential.key.as_ref().filter(|key| !key.is_empty())
					{
						return Ok(Some(AuthResult {
							auth: ModelAuth {
								api_key: Some(key.clone()),
								..ModelAuth::default()
							},
							env: credential.env.clone(),
							source: Some("stored credential".into()),
						}));
					}

					let auth_token = ctx.env(ANTHROPIC_AUTH_TOKEN_ENV).await;
					signal.throw_if_aborted()?;
					if let Some(auth_token) = auth_token.filter(|token| !token.is_empty()) {
						return Ok(Some(AuthResult {
							auth: ModelAuth {
								headers: Some(IndexMap::from([(
									"Authorization".to_string(),
									Some(format!("Bearer {auth_token}")),
								)])),
								..ModelAuth::default()
							},
							env: None,
							source: Some(ANTHROPIC_AUTH_TOKEN_ENV.into()),
						}));
					}

					for env_var in [ANTHROPIC_OAUTH_TOKEN_ENV, ANTHROPIC_API_KEY_ENV] {
						let api_key = ctx.env(env_var).await;
						signal.throw_if_aborted()?;
						if let Some(api_key) = api_key.filter(|key| !key.is_empty()) {
							return Ok(Some(AuthResult {
								auth: ModelAuth {
									api_key: Some(api_key),
									..ModelAuth::default()
								},
								env: None,
								source: Some(env_var.into()),
							}));
						}
					}

					// Workload identity federation: the Anthropic SDK exchanges the identity
					// token for a short-lived access token and refreshes it itself. Last in
					// line so keys and ANTHROPIC_AUTH_TOKEN keep winning, as in the SDK. The
					// ids are provider config rather than auth, so they travel in `env`.
					let mut federation = ProviderEnv::new();
					for env_var in [
						ANTHROPIC_FEDERATION_RULE_ID_ENV,
						ANTHROPIC_ORGANIZATION_ID_ENV,
						ANTHROPIC_IDENTITY_TOKEN_FILE_ENV,
					] {
						let value = ctx.env(env_var).await;
						signal.throw_if_aborted()?;
						let Some(value) = value.filter(|value| !value.is_empty()) else {
							return Ok(None);
						};
						federation.insert(env_var.into(), value);
					}
					for env_var in [ANTHROPIC_SERVICE_ACCOUNT_ID_ENV, ANTHROPIC_WORKSPACE_ID_ENV] {
						let value = ctx.env(env_var).await;
						signal.throw_if_aborted()?;
						if let Some(value) = value.filter(|value| !value.is_empty()) {
							federation.insert(env_var.into(), value);
						}
					}
					Ok(Some(AuthResult {
						auth: ModelAuth::default(),
						env: Some(federation),
						source: Some("workload identity federation".into()),
					}))
				})
			},
		),
	}
}

pub fn anthropic_provider() -> Provider {
	create_provider(CreateProviderOptions {
		id: "anthropic".into(),
		name: Some("Anthropic".into()),
		base_url: Some("https://api.anthropic.com".into()),
		auth: ProviderAuth {
			api_key: Some(anthropic_api_key_auth()),
			oauth: Some(lazy_oauth(LazyOAuthInput {
				name: "Anthropic (Claude Pro/Max)".into(),
				is_subscription: Some(true),
				login_label: None,
				load: Rc::new(|| Box::pin(load_anthropic_oauth())),
			})),
		},
		models: ANTHROPIC_MODELS.values().cloned().map(AnyModel::Chat).collect(),
		api: Some(ProviderApi::Single(anthropic_messages_api())),
		..CreateProviderOptions::default()
	})
	.expect("a provider with an api implementation always builds")
}
