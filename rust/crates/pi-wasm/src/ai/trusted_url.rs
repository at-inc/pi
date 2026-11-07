//! Exports of `auth/oauth/trusted-url`.

use wasm_bindgen::prelude::*;

/// `trustedHttpUrl(value)`: the normalized `http:`/`https:` URL, or `null`.
#[wasm_bindgen(js_name = aiTrustedHttpUrl)]
pub fn trusted_http_url(value: JsValue) -> JsValue {
	let value = crate::json::from_js::<serde_json::Value>(value).unwrap_or_default();
	pi_ai::auth::oauth::trusted_url::trusted_http_url(&value).map_or(JsValue::NULL, JsValue::from)
}
