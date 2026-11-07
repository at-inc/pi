//! Port of `packages/ai/src/auth/resolve.ts`.
//! PORT-STATUS: pending

// PORT-STUB: w2-ai-auth
use std::rc::Rc;

use pi_host::{AbortSignal, PiResult};

pub use crate::utils::models_error::{ModelsError, ModelsErrorCode};

use crate::auth::types::{AuthContext, AuthResult, CredentialStore, OAuthAuth, OAuthCredential, ProviderAuth};
use crate::types::ProviderEnv;

#[derive(Clone, Debug, Default)]
pub struct AuthResolutionOverrides {
	pub api_key: Option<String>,
	pub env: Option<ProviderEnv>,
	/// Require this much remaining OAuth-token validity; defaults to five minutes.
	pub min_oauth_validity_ms: Option<f64>,
	pub signal: Option<AbortSignal>,
}

/// Auth resolution shared by all operations in a `Models` collection. TS takes `provider: { id, auth }`.
pub async fn resolve_provider_auth(
	_provider_id: &str,
	_auth: &ProviderAuth,
	_credentials: Rc<dyn CredentialStore>,
	_auth_context: Rc<dyn AuthContext>,
	_overrides: Option<AuthResolutionOverrides>,
) -> PiResult<Option<AuthResult>> {
	todo!("PORT-STUB")
}

/// Refresh a stored OAuth credential under the credential-store lock and persist the result.
pub async fn refresh_stored_oauth_credential(
	_credentials: Rc<dyn CredentialStore>,
	_provider_id: &str,
	_oauth: &OAuthAuth,
	_needs_refresh: Rc<dyn Fn(&OAuthCredential) -> bool>,
	_signal: &AbortSignal,
) -> PiResult<Option<OAuthCredential>> {
	todo!("PORT-STUB")
}
