//! Exports of `utils/json-parse`. Parsed values cross as JSON text written without recursion (`json::value_to_js`),
//! so `-0` and deeply nested values survive. A lone surrogate in an argument string reads as U+FFFD.

use js_sys::{Object, Reflect};
use pi_ai::utils::json_parse::{self, ParsedToolCallArguments};
use serde_json::Value;
use wasm_bindgen::prelude::*;

use crate::json::value_to_js;
use crate::options::throw;

/// `repairJson(json)`.
#[wasm_bindgen(js_name = aiRepairJson)]
pub fn repair_json(json: &str) -> String {
	json_parse::repair_json(json)
}

/// `parseJsonWithRepair(json)`: throws the `SyntaxError` `JSON.parse` throws, with V8's message.
#[wasm_bindgen(js_name = aiParseJsonWithRepair)]
pub fn parse_json_with_repair(json: &str) -> Result<JsValue, JsValue> {
	throw(json_parse::parse_json_with_repair(json).and_then(|value| value_to_js(&value)))
}

/// `parseToolCallArguments(json)`.
#[wasm_bindgen(js_name = aiParseToolCallArguments)]
pub fn parse_tool_call_arguments(json: Option<String>) -> Result<JsValue, JsValue> {
	parsed_arguments_to_js(json_parse::parse_tool_call_arguments(json.as_deref()))
}

/// `parseStreamingJson(partialJson)`.
#[wasm_bindgen(js_name = aiParseStreamingJson)]
pub fn parse_streaming_json(partial_json: Option<String>) -> Result<JsValue, JsValue> {
	throw(value_to_js(&json_parse::parse_streaming_json(partial_json.as_deref())))
}

/// `{ arguments, argumentsError? }`.
pub(super) fn parsed_arguments_to_js(parsed: ParsedToolCallArguments) -> Result<JsValue, JsValue> {
	let result = Object::new();
	let arguments = throw(value_to_js(&Value::Object(parsed.arguments)))?;
	Reflect::set(&result, &"arguments".into(), &arguments)?;
	if let Some(error) = parsed.arguments_error {
		Reflect::set(&result, &"argumentsError".into(), &error.into())?;
	}
	Ok(result.into())
}
