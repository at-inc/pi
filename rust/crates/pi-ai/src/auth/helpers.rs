//! Port of `packages/ai/src/auth/helpers.ts`.
//! PORT-STATUS: pending

// PORT-STUB: w2-ai-auth
// The stubs build the auth objects so providers construct; their callbacks are todo.
use std::rc::Rc;

use pi_host::{LocalBoxFuture, PiResult};

use crate::auth::types::{ApiKeyAuth, OAuthAuth};

/// Standard api-key auth: a stored credential key wins, otherwise the first set env var resolves.
pub fn env_api_key_auth(name: &str, _env_vars: &[&str]) -> ApiKeyAuth {
	ApiKeyAuth {
		name: name.to_string(),
		login: Some(Rc::new(|_| todo!("PORT-STUB"))),
		check: None,
		resolve: Rc::new(|_| todo!("PORT-STUB")),
	}
}

/// The input of `lazyOAuth`.
#[derive(Clone)]
pub struct LazyOAuthInput {
	pub name: String,
	pub is_subscription: Option<bool>,
	pub login_label: Option<String>,
	pub load: Rc<dyn Fn() -> LocalBoxFuture<'static, PiResult<OAuthAuth>>>,
}

/// Wraps a lazily loaded `OAuthAuth` so provider definitions can advertise OAuth without loading the implementation.
pub fn lazy_oauth(input: LazyOAuthInput) -> OAuthAuth {
	OAuthAuth {
		name: input.name,
		is_subscription: input.is_subscription,
		login_label: input.login_label,
		login: Rc::new(|_, _| todo!("PORT-STUB")),
		refresh: Rc::new(|_, _| todo!("PORT-STUB")),
		to_auth: Rc::new(|_| todo!("PORT-STUB")),
	}
}
