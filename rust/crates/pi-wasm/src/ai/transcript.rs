//! Exports of `utils/transcript`.

use wasm_bindgen::prelude::*;

use crate::options::{arg, throw};

/// `normalizeContext(context)`.
#[wasm_bindgen(js_name = aiNormalizeContext)]
pub fn normalize_context(context: JsValue) -> Result<JsValue, JsValue> {
	let context: pi_ai::Context = arg(context)?;
	throw(crate::json::to_js(&pi_ai::normalize_context(&context)))
}
