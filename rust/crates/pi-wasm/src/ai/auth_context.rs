//! Exports of `auth/context`.

use wasm_bindgen::prelude::*;

use super::auth::AuthContextHandle;

/// `defaultProviderAuthContext()`.
#[wasm_bindgen(js_name = aiDefaultProviderAuthContext)]
pub fn default_provider_auth_context() -> AuthContextHandle {
	AuthContextHandle::new(pi_ai::auth::context::default_provider_auth_context())
}
