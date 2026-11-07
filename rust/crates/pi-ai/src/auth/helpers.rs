//! Port of `packages/ai/src/auth/helpers.ts`.
//! PORT-STATUS: ported
//!
//! `lazyOAuth`'s anonymous input object is `LazyOAuthInput`. The cached promise is a `Shared` future, so a rejected
//! load stays cached like the rejected TS promise.

use std::cell::RefCell;
use std::rc::Rc;

use futures::FutureExt;
use futures::future::Shared;
use pi_host::{LocalBoxFuture, PiResult};

use crate::auth::types::{
	ApiKeyAuth, ApiKeyAuthInput, ApiKeyCredential, AuthPrompt, AuthPromptKind, AuthResult, ModelAuth, OAuthAuth,
	ProviderAuthInteraction,
};

/// Standard api-key auth: a stored credential key wins, otherwise the first
/// set env var resolves. Includes a `login` that prompts for the key.
/// Providers with non-standard resolution (provider env, ambient files, IAM)
/// write their own `ApiKeyAuth`.
pub fn env_api_key_auth(name: &str, env_vars: &[&str]) -> ApiKeyAuth {
	let prompt_message = format!("Enter {name}");
	let env_vars: Rc<Vec<String>> = Rc::new(env_vars.iter().map(|env_var| env_var.to_string()).collect());
	ApiKeyAuth {
		name: name.to_string(),
		login: Some(Rc::new(move |interaction: ProviderAuthInteraction| {
			let prompt_message = prompt_message.clone();
			Box::pin(async move {
				interaction.signal.throw_if_aborted()?;
				let key = interaction
					.prompt(AuthPrompt::new(AuthPromptKind::Secret {
						message: prompt_message,
						placeholder: None,
					}))
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
			move |ApiKeyAuthInput {
			          ctx,
			          credential,
			          signal,
			      }| {
				let env_vars = env_vars.clone();
				Box::pin(async move {
					signal.throw_if_aborted()?;
					if let Some(credential) = credential
						&& let Some(key) = credential.key.filter(|key| !key.is_empty())
					{
						return Ok(Some(AuthResult {
							auth: ModelAuth {
								api_key: Some(key),
								..ModelAuth::default()
							},
							env: credential.env,
							source: Some("stored credential".into()),
						}));
					}
					for env_var in env_vars.iter() {
						let value = ctx.env(env_var).await;
						signal.throw_if_aborted()?;
						if let Some(value) = value.filter(|value| !value.is_empty()) {
							return Ok(Some(AuthResult {
								auth: ModelAuth {
									api_key: Some(value),
									..ModelAuth::default()
								},
								env: None,
								source: Some(env_var.clone()),
							}));
						}
					}
					Ok(None)
				})
			},
		),
	}
}

/// `lazyOAuth`'s `load`.
pub type LoadOAuthFunction = Rc<dyn Fn() -> LocalBoxFuture<'static, PiResult<OAuthAuth>>>;

/// The input of `lazyOAuth`.
#[derive(Clone)]
pub struct LazyOAuthInput {
	pub name: String,
	pub is_subscription: Option<bool>,
	pub login_label: Option<String>,
	pub load: LoadOAuthFunction,
}

type LoadedOAuth = Shared<LocalBoxFuture<'static, PiResult<OAuthAuth>>>;

/// Wraps a dynamically imported `OAuthAuth` so provider definitions can
/// advertise OAuth without importing the implementation. The flow loads on
/// first `login`/`refresh`/`toAuth` call; callers keep Node-only flow code out
/// of bundles by loading through a bundler-opaque dynamic import (variable
/// specifier, see the bedrock lazy wrapper).
pub fn lazy_oauth(input: LazyOAuthInput) -> OAuthAuth {
	let promise: Rc<RefCell<Option<LoadedOAuth>>> = Rc::new(RefCell::new(None));
	let load = input.load;
	let loaded = Rc::new(move || promise.borrow_mut().get_or_insert_with(|| load().shared()).clone());
	let login_loaded = loaded.clone();
	let refresh_loaded = loaded.clone();
	OAuthAuth {
		name: input.name,
		is_subscription: input.is_subscription,
		login_label: input.login_label,
		login: Rc::new(move |interaction, options| {
			let loaded = login_loaded();
			Box::pin(async move { loaded.await?.login(interaction, options).await })
		}),
		refresh: Rc::new(move |credential, signal| {
			let loaded = refresh_loaded();
			Box::pin(async move { loaded.await?.refresh(credential, signal).await })
		}),
		to_auth: Rc::new(move |credential| {
			let loaded = loaded();
			Box::pin(async move { loaded.await?.to_auth(credential).await })
		}),
	}
}
