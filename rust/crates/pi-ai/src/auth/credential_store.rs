//! Port of `packages/ai/src/auth/credential-store.ts`.
//! PORT-STATUS: pending

// PORT-STUB: w2-ai-auth
use pi_host::{LocalBoxFuture, PiResult};

use crate::auth::types::{AuthOperationOptions, Credential, CredentialInfo, CredentialModifier, CredentialStore};

/// Default in-memory credential store.
#[derive(Default)]
pub struct InMemoryCredentialStore {}

impl InMemoryCredentialStore {
	/// `new InMemoryCredentialStore()`.
	pub fn new() -> Self {
		Self::default()
	}
}

impl CredentialStore for InMemoryCredentialStore {
	fn read(
		&self,
		_provider_id: &str,
		_options: Option<AuthOperationOptions>,
	) -> LocalBoxFuture<'static, PiResult<Option<Credential>>> {
		todo!("PORT-STUB")
	}

	fn list(&self, _options: Option<AuthOperationOptions>) -> LocalBoxFuture<'static, PiResult<Vec<CredentialInfo>>> {
		todo!("PORT-STUB")
	}

	fn modify(
		&self,
		_provider_id: &str,
		_f: CredentialModifier,
		_options: Option<AuthOperationOptions>,
	) -> LocalBoxFuture<'static, PiResult<Option<Credential>>> {
		todo!("PORT-STUB")
	}

	fn delete(
		&self,
		_provider_id: &str,
		_options: Option<AuthOperationOptions>,
	) -> LocalBoxFuture<'static, PiResult<()>> {
		todo!("PORT-STUB")
	}
}
