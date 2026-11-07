//! Exports of `utils/kimi-coding`.

use js_sys::{Object, Reflect};
use pi_ai::utils::kimi_coding::{self, KimiCodingEndpointsInput};
use wasm_bindgen::prelude::*;

use crate::options::throw;

/// `kimiCodingEndpoints(input)`.
#[wasm_bindgen(js_name = aiKimiCodingEndpoints)]
pub fn kimi_coding_endpoints(input: JsValue) -> Result<JsValue, JsValue> {
	let field = |name: &str| -> Result<Option<String>, JsValue> { Ok(Reflect::get(&input, &name.into())?.as_string()) };
	let endpoints = throw(kimi_coding::kimi_coding_endpoints(KimiCodingEndpointsInput {
		oauth_host: field("oauthHost")?,
		base_url: field("baseUrl")?,
	}))?;
	let result = Object::new();
	Reflect::set(&result, &"oauthHost".into(), &endpoints.oauth_host.into())?;
	Reflect::set(&result, &"baseUrl".into(), &endpoints.base_url.into())?;
	Ok(result.into())
}
