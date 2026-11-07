//! Port of `packages/ai/src/providers/kimi-coding.ts`.
//! PORT-STATUS: reviewed
//!
//! Differences from the TS:
//! - `Provider<TApi>`'s type parameter only narrows the API type, so the factory returns a plain `Provider`.
//! - `createProvider()` throws only without an implementation, which this factory always passes, so the factory
//!   returns the provider instead of a `PiResult`.
//! - `{ ...apiKeyAuth, login, resolve }` copies `name` and `check` from the env-var auth explicitly.

use std::rc::Rc;

use crate::api::openai_completions_lazy::openai_completions_api;
use crate::auth::helpers::{LazyOAuthInput, env_api_key_auth, lazy_oauth};
use crate::auth::oauth::load::load_kimi_coding_oauth;
use crate::auth::types::{ApiKeyAuth, ApiKeyAuthInput, ApiKeyCredential, AuthResult, ModelAuth, ProviderAuth};
use crate::models::{CreateProviderOptions, Provider, ProviderApi, create_provider};
use crate::providers::kimi_coding_models::KIMI_CODING_MODELS;
use crate::types::{AnyModel, ProviderEnv};
use crate::utils::kimi_coding::{KimiCodingEndpointsInput, kimi_coding_endpoints, select_kimi_coding_endpoints};

pub fn kimi_coding_provider() -> Provider {
	let api_key_auth = env_api_key_auth("Kimi API key", &["KIMI_API_KEY"]);
	let login = api_key_auth.login.clone();
	let resolve = api_key_auth.resolve.clone();
	create_provider(CreateProviderOptions {
		id: "kimi-coding".into(),
		name: Some("Kimi For Coding".into()),
		base_url: Some("https://api.kimi.com/coding/v1".into()),
		auth: ProviderAuth {
			api_key: Some(ApiKeyAuth {
				name: api_key_auth.name.clone(),
				check: api_key_auth.check.clone(),
				login: Some(Rc::new(move |interaction| {
					let login = login.clone().expect("envApiKeyAuth always provides login");
					Box::pin(async move {
						let endpoints = select_kimi_coding_endpoints(&interaction).await?;
						let credential = login(interaction).await?;
						Ok(ApiKeyCredential {
							env: Some(ProviderEnv::from([
								("KIMI_CODE_OAUTH_HOST".to_string(), endpoints.oauth_host),
								("KIMI_CODE_BASE_URL".to_string(), endpoints.base_url),
							])),
							..credential
						})
					})
				})),
				resolve: Rc::new(move |input: ApiKeyAuthInput| {
					let resolve = resolve.clone();
					Box::pin(async move {
						let Some(result) = resolve(input.clone()).await? else {
							return Ok(None);
						};
						let stored = |name: &str| {
							input
								.credential
								.as_ref()
								.and_then(|credential| credential.env.as_ref())
								.and_then(|env| env.get(name))
								.filter(|value| !value.is_empty())
								.cloned()
						};
						let ambient = |value: Option<String>| value.filter(|value| !value.is_empty());
						let oauth_host = match stored("KIMI_CODE_OAUTH_HOST").or_else(|| stored("KIMI_OAUTH_HOST")) {
							Some(oauth_host) => Some(oauth_host),
							None => match ambient(input.ctx.env("KIMI_CODE_OAUTH_HOST").await) {
								Some(oauth_host) => Some(oauth_host),
								None => ambient(input.ctx.env("KIMI_OAUTH_HOST").await),
							},
						};
						let base_url = match stored("KIMI_CODE_BASE_URL") {
							Some(base_url) => Some(base_url),
							None => ambient(input.ctx.env("KIMI_CODE_BASE_URL").await),
						};
						input.signal.throw_if_aborted()?;
						if oauth_host.is_none() && base_url.is_none() {
							return Ok(Some(result));
						}
						let base_url =
							kimi_coding_endpoints(KimiCodingEndpointsInput { oauth_host, base_url })?.base_url;
						Ok(Some(AuthResult {
							auth: ModelAuth {
								base_url: Some(base_url),
								..result.auth
							},
							..result
						}))
					})
				}),
			}),
			oauth: Some(lazy_oauth(LazyOAuthInput {
				name: "Kimi Code (subscription)".into(),
				is_subscription: Some(true),
				login_label: Some("Sign in with Kimi Code".into()),
				load: Rc::new(|| Box::pin(load_kimi_coding_oauth())),
			})),
		},
		models: KIMI_CODING_MODELS.values().cloned().map(AnyModel::Chat).collect(),
		api: Some(ProviderApi::Single(openai_completions_api())),
		..CreateProviderOptions::default()
	})
	.expect("a provider with an api implementation always builds")
}
