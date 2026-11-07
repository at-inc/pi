//! Exports of `protocol/content`: `toLlmContent`.

use pi_mcp::CallToolResult;
use wasm_bindgen::prelude::*;

use crate::options::{arg, throw};

/// `toLlmContent(result)`.
#[wasm_bindgen(js_name = mcpToLlmContent)]
pub fn to_llm_content(result: JsValue) -> Result<JsValue, JsValue> {
	let result: CallToolResult = arg(result)?;
	throw(crate::json::to_js(&pi_mcp::to_llm_content(&result)))
}
