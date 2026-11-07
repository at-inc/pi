//! Port of `packages/ai/src/auth/context.ts`.
//! PORT-STATUS: pending

// PORT-STUB: w2-ai-auth
use std::rc::Rc;

use pi_host::LocalBoxFuture;

use crate::auth::types::AuthContext;

struct DefaultProviderAuthContext;

impl AuthContext for DefaultProviderAuthContext {
	fn env(&self, _name: &str) -> LocalBoxFuture<'static, Option<String>> {
		todo!("PORT-STUB")
	}

	fn file_exists(&self, _path: &str) -> LocalBoxFuture<'static, bool> {
		todo!("PORT-STUB")
	}
}

/// Default auth context: env vars from `process.env`, file existence via the host.
pub fn default_provider_auth_context() -> Rc<dyn AuthContext> {
	Rc::new(DefaultProviderAuthContext)
}
