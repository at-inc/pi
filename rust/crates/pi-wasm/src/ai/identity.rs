//! Exports of `utils/identity`. Identities and targets cross as JSON text.

use js_sys::Reflect;
use pi_ai::utils::identity::{self, ClientIdentity, ClientIdentityTarget, PI_ATTRIBUTION};
use wasm_bindgen::prelude::*;

use crate::json::{from_js_text, to_js_text};
use crate::options::throw;

/// `PI_ATTRIBUTION`.
#[wasm_bindgen(js_name = aiPiAttribution)]
pub fn pi_attribution() -> Result<JsValue, JsValue> {
	throw(to_js_text(&*PI_ATTRIBUTION))
}

/// `mergeClientIdentity(...sources)`.
#[wasm_bindgen(js_name = aiMergeClientIdentity)]
pub fn merge_client_identity(sources: JsValue) -> Result<JsValue, JsValue> {
	let sources: Vec<Option<ClientIdentity>> = throw(from_js_text(&sources))?;
	let sources: Vec<Option<&ClientIdentity>> = sources.iter().map(Option::as_ref).collect();
	match identity::merge_client_identity(&sources) {
		Some(merged) => throw(to_js_text(&merged)),
		None => Ok(JsValue::UNDEFINED),
	}
}

/// `getClientVersion(identity)`.
#[wasm_bindgen(js_name = aiGetClientVersion)]
pub fn get_client_version(identity: JsValue) -> Result<String, JsValue> {
	Ok(identity::get_client_version(client_identity(&identity)?.as_ref()))
}

/// `formatUserAgent(identity)`.
#[wasm_bindgen(js_name = aiFormatUserAgent)]
pub fn format_user_agent(identity: JsValue) -> Result<String, JsValue> {
	Ok(identity::format_user_agent(client_identity(&identity)?.as_ref()))
}

/// `getAttributionHeaders(target, identity)`.
#[wasm_bindgen(js_name = aiGetAttributionHeaders)]
pub fn get_attribution_headers(target: JsValue, identity: JsValue) -> Result<JsValue, JsValue> {
	let (provider, base_url) = target_fields(&target)?;
	let target = ClientIdentityTarget {
		provider: &provider,
		base_url: &base_url,
	};
	let headers = throw(identity::get_attribution_headers(
		target,
		client_identity(&identity)?.as_ref(),
	))?;
	throw(to_js_text(&headers))
}

/// `getOpenCodeSessionHeaders(target, sessionId)`.
#[wasm_bindgen(js_name = aiGetOpenCodeSessionHeaders)]
pub fn get_open_code_session_headers(target: JsValue, session_id: Option<String>) -> Result<JsValue, JsValue> {
	let (provider, base_url) = target_fields(&target)?;
	let target = ClientIdentityTarget {
		provider: &provider,
		base_url: &base_url,
	};
	match identity::get_open_code_session_headers(target, session_id.as_deref()) {
		Some(headers) => throw(to_js_text(&headers)),
		None => Ok(JsValue::UNDEFINED),
	}
}

/// `getIdentityHeaders(target, identity)`.
#[wasm_bindgen(js_name = aiGetIdentityHeaders)]
pub fn get_identity_headers(target: JsValue, identity: JsValue) -> Result<JsValue, JsValue> {
	let (provider, base_url) = target_fields(&target)?;
	let target = ClientIdentityTarget {
		provider: &provider,
		base_url: &base_url,
	};
	let headers = throw(identity::get_identity_headers(
		target,
		client_identity(&identity)?.as_ref(),
	))?;
	throw(to_js_text(&headers))
}

/// An optional identity argument.
fn client_identity(identity: &JsValue) -> Result<Option<ClientIdentity>, JsValue> {
	throw(from_js_text(identity))
}

/// A target's `provider` and `baseUrl`, read as strings.
fn target_fields(target: &JsValue) -> Result<(String, String), JsValue> {
	let field = |name: &str| -> Result<String, JsValue> {
		Ok(Reflect::get(target, &name.into())?.as_string().unwrap_or_default())
	};
	Ok((field("provider")?, field("baseUrl")?))
}
