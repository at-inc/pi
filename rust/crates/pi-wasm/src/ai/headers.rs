//! Exports of `utils/headers`.

use indexmap::IndexMap;
use js_sys::Object;
use pi_ai::ProviderHeaders;
use pi_ai::utils::headers;
use wasm_bindgen::prelude::*;

use crate::json::{from_js_text, to_js_text};
use crate::options::throw;

/// `headersToRecord(headers)`: the entries of a `Headers` object.
#[wasm_bindgen(js_name = aiHeadersToRecord)]
pub fn headers_to_record(headers: JsValue) -> Result<JsValue, JsValue> {
	let entries: IndexMap<String, String> = throw(from_js_text(&Object::from_entries(&headers)?.into()))?;
	throw(to_js_text(&headers::headers_to_record(&entries)))
}

/// `providerHeadersToRecord(...headerSources)`.
#[wasm_bindgen(js_name = aiProviderHeadersToRecord)]
pub fn provider_headers_to_record(header_sources: JsValue) -> Result<JsValue, JsValue> {
	let sources: Vec<Option<ProviderHeaders>> = throw(from_js_text(&header_sources))?;
	let sources: Vec<Option<&ProviderHeaders>> = sources.iter().map(Option::as_ref).collect();
	match headers::provider_headers_to_record(&sources) {
		Some(record) => throw(to_js_text(&record)),
		None => Ok(JsValue::UNDEFINED),
	}
}
