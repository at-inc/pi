//! Port of `packages/ai/src/auth/resolve.ts`.
//! PORT-STATUS: ported
//!
//! TS takes `provider: { id: string; auth: ProviderAuth }`; the port takes `provider_id` and `auth`. Both public
//! functions do their synchronous TS prefix (signal setup, starting the race or the store call) when called and
//! return a `'static` future, like the TS functions that return promises.

use std::future::Future;
use std::rc::Rc;

use pi_host::{AbortController, AbortSignal, LocalBoxFuture, PiError, PiResult};

pub use crate::utils::models_error::{ModelsError, ModelsErrorCode};

use crate::auth::types::{
	ApiKeyAuth, ApiKeyAuthInput, ApiKeyCredential, AuthContext, AuthOperationOptions, AuthResult, Credential,
	CredentialStore, OAuthAuth, OAuthCredential, ProviderAuth,
};
use crate::types::ProviderEnv;
use crate::utils::abort::{operation_signal, race_with_abort_signal};

#[derive(Clone, Debug, Default)]
pub struct AuthResolutionOverrides {
	pub api_key: Option<String>,
	pub env: Option<ProviderEnv>,
	/// Require this much remaining OAuth-token validity; defaults to five minutes.
	pub min_oauth_validity_ms: Option<f64>,
	pub signal: Option<AbortSignal>,
}

/// `needsRefresh` of `refreshStoredOAuthCredential`.
pub type NeedsRefreshFunction = Rc<dyn Fn(&OAuthCredential) -> bool>;

/// Auth resolution shared by all operations in a `Models` collection.
/// A stored credential owns the provider: ambient/env is consulted only when
/// nothing is stored. No silent env fallback after a failed refresh or for a
/// credential type without a matching handler.
pub fn resolve_provider_auth(
	provider_id: &str,
	auth: &ProviderAuth,
	credentials: Rc<dyn CredentialStore>,
	auth_context: Rc<dyn AuthContext>,
	overrides: Option<AuthResolutionOverrides>,
) -> impl Future<Output = PiResult<Option<AuthResult>>> + 'static + use<> {
	let signal = operation_signal(overrides.as_ref().and_then(|overrides| overrides.signal.as_ref()));
	race_with_abort_signal(
		resolve_provider_auth_with_signal(
			provider_id.to_string(),
			auth.clone(),
			credentials,
			auth_context,
			overrides,
			signal.clone(),
		),
		&signal,
	)
}

async fn resolve_provider_auth_with_signal(
	provider_id: String,
	auth: ProviderAuth,
	credentials: Rc<dyn CredentialStore>,
	auth_context: Rc<dyn AuthContext>,
	overrides: Option<AuthResolutionOverrides>,
	signal: AbortSignal,
) -> PiResult<Option<AuthResult>> {
	signal.throw_if_aborted()?;
	let override_env = overrides.as_ref().and_then(|overrides| overrides.env.clone());
	let request_auth_context = match &override_env {
		Some(env) => overlay_env_auth_context(auth_context, env.clone()),
		None => auth_context,
	};

	if let Some(overrides) = &overrides
		&& let Some(api_key_override) = &overrides.api_key
		&& let Some(api_key) = &auth.api_key
	{
		return resolve_api_key(
			request_auth_context,
			api_key,
			&provider_id,
			Some(ApiKeyCredential {
				key: Some(api_key_override.clone()),
				env: overrides.env.clone(),
			}),
			signal,
		)
		.await;
	}

	let stored = read_credential(credentials.clone(), &provider_id, signal.clone()).await?;
	if let Some(stored) = stored {
		match (stored, &auth.oauth, &auth.api_key) {
			(Credential::OAuth(stored), Some(oauth), _) => {
				return resolve_stored_oauth(
					credentials,
					&provider_id,
					oauth,
					stored,
					signal,
					overrides.as_ref().and_then(|overrides| overrides.min_oauth_validity_ms),
				)
				.await;
			}
			(Credential::ApiKey(stored), _, Some(api_key)) => {
				let credential = match override_env {
					Some(override_env) => {
						let mut env = stored.env.clone().unwrap_or_default();
						env.extend(override_env);
						ApiKeyCredential {
							env: Some(env),
							..stored
						}
					}
					None => stored,
				};
				return resolve_api_key(request_auth_context, api_key, &provider_id, Some(credential), signal).await;
			}
			_ => return Ok(None),
		}
	}

	// Ambient (env vars, AWS profiles, ADC files).
	match &auth.api_key {
		Some(api_key) => resolve_api_key(request_auth_context, api_key, &provider_id, None, signal).await,
		None => Ok(None),
	}
}

struct OverlayEnvAuthContext {
	base: Rc<dyn AuthContext>,
	env: ProviderEnv,
}

impl AuthContext for OverlayEnvAuthContext {
	fn env(&self, name: &str) -> LocalBoxFuture<'static, Option<String>> {
		if let Some(value) = self.env.get(name).filter(|value| !value.is_empty()) {
			let value = value.clone();
			return Box::pin(async move { Some(value) });
		}
		self.base.env(name)
	}

	fn file_exists(&self, path: &str) -> LocalBoxFuture<'static, bool> {
		self.base.file_exists(path)
	}
}

fn overlay_env_auth_context(base: Rc<dyn AuthContext>, env: ProviderEnv) -> Rc<dyn AuthContext> {
	Rc::new(OverlayEnvAuthContext { base, env })
}

const DEFAULT_OAUTH_MINIMUM_VALIDITY_MS: f64 = 5.0 * 60.0 * 1000.0;
const DEFAULT_OAUTH_REFRESH_TIMEOUT_MS: f64 = 15_000.0;

/// Refresh a stored OAuth credential under the credential-store lock and persist
/// the result before the lock is released. `needsRefresh` is re-checked under
/// the lock, so concurrent callers and processes refresh only once.
///
/// `signal` cancels only the wait for the lock. Once a refresh starts, the
/// provider may already have rotated the refresh token, so the refresh and its
/// persistence ignore `signal` and are bounded only by a timeout. Otherwise a
/// cancelled caller could discard the only valid refresh token. Callers that
/// must return promptly on cancellation race this promise with their signal.
///
/// Resolves with the stored OAuth credential after the operation, or undefined
/// when the provider no longer has an OAuth credential.
pub fn refresh_stored_oauth_credential(
	credentials: Rc<dyn CredentialStore>,
	provider_id: &str,
	oauth: &OAuthAuth,
	needs_refresh: NeedsRefreshFunction,
	signal: &AbortSignal,
) -> impl Future<Output = PiResult<Option<OAuthCredential>>> + 'static + use<> {
	let lock_wait = AbortController::new();
	let cancel_lock_wait = {
		let lock_wait = lock_wait.clone();
		move |reason: &PiError| lock_wait.abort_with(reason.clone())
	};
	let listener = signal.add_listener(cancel_lock_wait.clone());
	if let Some(reason) = signal.reason() {
		cancel_lock_wait(&reason);
	}
	let modified = {
		let signal = signal.clone();
		let oauth = oauth.clone();
		let provider_id = provider_id.to_string();
		credentials.modify(
			&provider_id.clone(),
			Box::new(move |current| {
				Box::pin(async move {
					signal.remove_listener(listener);
					signal.throw_if_aborted()?;
					let Some(Credential::OAuth(current)) = current else {
						return Ok(None); // logged out meanwhile
					};
					if !needs_refresh(&current) {
						return Ok(None); // another process/request refreshed
					}
					match oauth
						.refresh(current, AbortSignal::timeout(DEFAULT_OAUTH_REFRESH_TIMEOUT_MS))
						.await
					{
						Ok(refreshed) => Ok(Some(Credential::OAuth(refreshed))),
						Err(error) => Err(ModelsError::new(
							ModelsErrorCode::Oauth,
							&format!("OAuth refresh failed for {provider_id}"),
							Some((&error).into()),
						)),
					}
				})
			}),
			Some(AuthOperationOptions {
				signal: Some(lock_wait.signal()),
			}),
		)
	};
	let signal = signal.clone();
	let provider_id = provider_id.to_string();
	async move {
		let post = modified.await;
		signal.remove_listener(listener);
		match post {
			Ok(Some(Credential::OAuth(post))) => Ok(Some(post)),
			Ok(_) => Ok(None),
			Err(error) => {
				if ModelsError::is(&error) {
					return Err(error);
				}
				signal.throw_if_aborted()?;
				Err(ModelsError::new(
					ModelsErrorCode::Auth,
					&format!("Credential store modify failed for {provider_id}"),
					Some((&error).into()),
				))
			}
		}
	}
}

/// OAuth resolution with double-checked locking: tokens with less than five
/// minutes remaining are refreshed through `refreshStoredOAuthCredential`.
async fn resolve_stored_oauth(
	credentials: Rc<dyn CredentialStore>,
	provider_id: &str,
	oauth: &OAuthAuth,
	stored: OAuthCredential,
	signal: AbortSignal,
	min_oauth_validity_ms: Option<f64>,
) -> PiResult<Option<AuthResult>> {
	let minimum_validity_ms = DEFAULT_OAUTH_MINIMUM_VALIDITY_MS.max(min_oauth_validity_ms.unwrap_or(0.0));
	let expires_soon =
		move |credential: &OAuthCredential| pi_host::now_ms() + minimum_validity_ms >= credential.expires;
	let mut credential = stored;

	if expires_soon(&credential) {
		// Optimistic check said expired; the authoritative check runs under the lock.
		let post =
			refresh_stored_oauth_credential(credentials, provider_id, oauth, Rc::new(expires_soon), &signal).await?;
		let Some(post) = post else {
			return Ok(None); // logged out meanwhile
		};
		credential = post;
		// The normal five-minute window triggers a refresh but does not impose a
		// provider contract. Explicit callers (such as bearer-token export) do
		// require the requested minimum after the refresh.
		if min_oauth_validity_ms.is_some() && expires_soon(&credential) {
			return Err(ModelsError::new(
				ModelsErrorCode::Oauth,
				&format!("OAuth refresh returned a token that expires too soon for {provider_id}"),
				None,
			));
		}
	}

	match oauth.to_auth(credential).await {
		Ok(auth) => Ok(Some(AuthResult {
			auth,
			env: None,
			source: Some("OAuth".into()),
		})),
		Err(error) => Err(ModelsError::new(
			ModelsErrorCode::Oauth,
			&format!("OAuth auth derivation failed for {provider_id}"),
			Some((&error).into()),
		)),
	}
}

async fn resolve_api_key(
	auth_context: Rc<dyn AuthContext>,
	api_key: &ApiKeyAuth,
	provider_id: &str,
	credential: Option<ApiKeyCredential>,
	signal: AbortSignal,
) -> PiResult<Option<AuthResult>> {
	api_key
		.resolve(ApiKeyAuthInput {
			ctx: auth_context,
			credential,
			signal,
		})
		.await
		.map_err(|error| {
			ModelsError::new(
				ModelsErrorCode::Auth,
				&format!("API key auth failed for provider {provider_id}"),
				Some((&error).into()),
			)
		})
}

async fn read_credential(
	credentials: Rc<dyn CredentialStore>,
	provider_id: &str,
	signal: AbortSignal,
) -> PiResult<Option<Credential>> {
	credentials
		.read(provider_id, Some(AuthOperationOptions { signal: Some(signal) }))
		.await
		.map_err(|error| {
			ModelsError::new(
				ModelsErrorCode::Auth,
				&format!("Credential store read failed for {provider_id}"),
				Some((&error).into()),
			)
		})
}
