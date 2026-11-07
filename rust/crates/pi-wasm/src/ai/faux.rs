//! Exports of `providers/faux`: the message builders. Blocks cross as JSON text.

use js_sys::Reflect;
use pi_ai::providers::faux::{self, FauxAssistantContent, FauxAssistantMessageOptions, FauxToolCallOptions};
use pi_ai::{AssistantContent, DeferredHandle, JsonObject, StopReason};
use serde::Deserialize;
use wasm_bindgen::prelude::*;

use crate::json::{from_js_text, to_js_text};
use crate::options::throw;

/// `fauxText(text)`.
#[wasm_bindgen(js_name = aiFauxText)]
pub fn faux_text(text: &str) -> Result<JsValue, JsValue> {
	throw(to_js_text(&AssistantContent::Text(faux::faux_text(text))))
}

/// `fauxThinking(thinking)`.
#[wasm_bindgen(js_name = aiFauxThinking)]
pub fn faux_thinking(thinking: &str) -> Result<JsValue, JsValue> {
	throw(to_js_text(&AssistantContent::Thinking(faux::faux_thinking(thinking))))
}

/// `fauxToolCall(name, arguments, options)`.
#[wasm_bindgen(js_name = aiFauxToolCall)]
pub fn faux_tool_call(name: &str, arguments: JsValue, options: JsValue) -> Result<JsValue, JsValue> {
	let arguments: JsonObject = throw(from_js_text(&arguments))?;
	let id = if options.is_object() {
		Reflect::get(&options, &"id".into())?.as_string()
	} else {
		None
	};
	let call = faux::faux_tool_call(name, arguments, Some(FauxToolCallOptions { id }));
	throw(to_js_text(&AssistantContent::ToolCall(call)))
}

/// `fauxAssistantMessage(content, options)`: `content` is a string, a block or a block list.
#[wasm_bindgen(js_name = aiFauxAssistantMessage)]
pub fn faux_assistant_message(content: JsValue, options: JsValue) -> Result<JsValue, JsValue> {
	let content = if let Some(text) = content.as_string() {
		FauxAssistantContent::Text(text)
	} else if js_sys::Array::is_array(&content) {
		FauxAssistantContent::Blocks(throw(from_js_text(&content))?)
	} else {
		FauxAssistantContent::Block(throw(from_js_text(&content))?)
	};
	let options: Option<Options> = throw(from_js_text(&options))?;
	let options = options.map(|options| FauxAssistantMessageOptions {
		stop_reason: options.stop_reason,
		deferred: options.deferred,
		error_message: options.error_message,
		response_id: options.response_id,
		timestamp: options.timestamp,
	});
	throw(to_js_text(&faux::faux_assistant_message(content, options)))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Options {
	stop_reason: Option<StopReason>,
	deferred: Option<DeferredHandle>,
	error_message: Option<String>,
	response_id: Option<String>,
	timestamp: Option<f64>,
}
