//! Exports of `utils/node-http-proxy`.

use js_sys::{Array, Function, Reflect};
use pi_ai::ProviderEnv;
use pi_ai::utils::node_http_proxy;
use wasm_bindgen::JsCast;
use wasm_bindgen::prelude::*;

use crate::json::from_js_text;
use crate::options::throw;

#[wasm_bindgen]
extern "C" {
	/// `String(value)`.
	#[wasm_bindgen(js_name = String)]
	fn js_string(value: &JsValue) -> String;
}

/// `UNSUPPORTED_PROXY_PROTOCOL_MESSAGE`.
#[wasm_bindgen(js_name = aiUnsupportedProxyProtocolMessage)]
pub fn unsupported_proxy_protocol_message() -> String {
	node_http_proxy::UNSUPPORTED_PROXY_PROTOCOL_MESSAGE.into()
}

/// `resolveHttpProxyUrlForTarget(targetUrl, env)`: a new `URL` of the proxy, or `undefined`.
#[wasm_bindgen(js_name = aiResolveHttpProxyUrlForTarget)]
pub fn resolve_http_proxy_url_for_target(target_url: JsValue, env: JsValue) -> Result<JsValue, JsValue> {
	let env: Option<ProviderEnv> = throw(from_js_text(&env))?;
	match throw(node_http_proxy::resolve_http_proxy_url_for_target(
		&js_string(&target_url),
		env.as_ref(),
	))? {
		Some(url) => Ok(new_url(url.as_str())?),
		None => Ok(JsValue::UNDEFINED),
	}
}

/// `new URL(href)`.
fn new_url(href: &str) -> Result<JsValue, JsValue> {
	let constructor: Function = Reflect::get(&js_sys::global(), &"URL".into())?.dyn_into()?;
	Reflect::construct(&constructor, &Array::of1(&href.into()))
}
