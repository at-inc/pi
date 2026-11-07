//! Exports of `auth/resolve`: `resolveProviderAuth` and `refreshStoredOAuthCredential` over JS stores and auth.

use std::rc::Rc;

use js_sys::{Function, Promise, Reflect};
use pi_ai::auth::resolve::{AuthResolutionOverrides, refresh_stored_oauth_credential, resolve_provider_auth};
use pi_ai::auth::types::OAuthCredential;
use wasm_bindgen::prelude::*;

use super::auth::{
	auth_context_from_js, credential_store_from_js, oauth_auth_from_js, optional_field, promise, provider_auth,
};
use crate::options::arg;

/// `resolveProviderAuth(provider, credentials, authContext, overrides)`.
#[wasm_bindgen(js_name = aiResolveProviderAuth)]
pub fn ai_resolve_provider_auth(
	provider: JsValue,
	credentials: JsValue,
	auth_context: JsValue,
	overrides: JsValue,
) -> Result<Promise, JsValue> {
	let field = |object: &JsValue, name: &str| Reflect::get(object, &name.into()).unwrap_or(JsValue::UNDEFINED);
	let id: String = arg(field(&provider, "id"))?;
	let overrides = if overrides.is_object() {
		Some(AuthResolutionOverrides {
			api_key: optional_field(&overrides, "apiKey")?,
			env: optional_field(&overrides, "env")?,
			min_oauth_validity_ms: optional_field(&overrides, "minOAuthValidityMs")?,
			signal: crate::abort::from_js(&field(&overrides, "signal")),
		})
	} else {
		None
	};
	let resolved = resolve_provider_auth(
		&id,
		&provider_auth(&field(&provider, "auth")),
		credential_store_from_js(&credentials),
		auth_context_from_js(&auth_context),
		overrides,
	);
	Ok(promise(async move {
		resolved
			.await?
			.map_or(Ok(JsValue::UNDEFINED), |result| crate::json::to_js(&result))
	}))
}

/// `refreshStoredOAuthCredential(credentials, providerId, oauth, needsRefresh, signal)`.
#[wasm_bindgen(js_name = aiRefreshStoredOAuthCredential)]
pub fn ai_refresh_stored_oauth_credential(
	credentials: JsValue,
	provider_id: &str,
	oauth: JsValue,
	needs_refresh: Function,
	signal: JsValue,
) -> Promise {
	let needs_refresh = Rc::new(move |credential: &OAuthCredential| {
		crate::json::to_js(credential)
			.ok()
			.and_then(|credential| needs_refresh.call1(&JsValue::UNDEFINED, &credential).ok())
			.is_some_and(|result| result.is_truthy())
	});
	let signal = crate::abort::from_js(&signal).unwrap_or_else(|| pi_host::AbortController::new().signal());
	let refreshed = refresh_stored_oauth_credential(
		credential_store_from_js(&credentials),
		provider_id,
		&oauth_auth_from_js(&oauth),
		needs_refresh,
		&signal,
	);
	promise(async move {
		refreshed
			.await?
			.map_or(Ok(JsValue::UNDEFINED), |credential| crate::json::to_js(&credential))
	})
}
