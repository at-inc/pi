//! Exports of `auth/helpers`.

use std::rc::Rc;

use js_sys::{Function, Reflect};
use pi_ai::auth::helpers::{LazyOAuthInput, env_api_key_auth, lazy_oauth};
use wasm_bindgen::JsCast;
use wasm_bindgen::prelude::*;

use super::auth::{ApiKeyAuthHandle, OAuthAuthHandle, loaded_oauth_auth};

/// `envApiKeyAuth(name, envVars)`.
#[wasm_bindgen(js_name = aiEnvApiKeyAuth)]
pub fn ai_env_api_key_auth(name: &str, env_vars: Vec<String>) -> ApiKeyAuthHandle {
	let env_vars: Vec<&str> = env_vars.iter().map(String::as_str).collect();
	ApiKeyAuthHandle::new(env_api_key_auth(name, &env_vars))
}

/// `lazyOAuth({ name, isSubscription, loginLabel, load })`. `load` is called on the first `login`, `refresh` or
/// `toAuth` and its result is kept, a rejection included.
#[wasm_bindgen(js_name = aiLazyOAuth)]
pub fn ai_lazy_oauth(input: JsValue) -> Result<OAuthAuthHandle, JsValue> {
	let field = |name: &str| Reflect::get(&input, &name.into()).unwrap_or(JsValue::UNDEFINED);
	let load: Function = field("load")
		.dyn_into()
		.map_err(|_| JsValue::from(js_sys::TypeError::new("input.load is not a function")))?;
	Ok(OAuthAuthHandle::new(lazy_oauth(LazyOAuthInput {
		name: field("name").as_string().unwrap_or_default(),
		is_subscription: field("isSubscription").as_bool(),
		login_label: field("loginLabel").as_string(),
		load: Rc::new(move || {
			let loaded = load.call0(&JsValue::UNDEFINED).map_err(crate::error::from_js);
			Box::pin(async move {
				let auth = loaded_oauth_auth(loaded?);
				Ok(auth)
			})
		}),
	})))
}
