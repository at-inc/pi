//! Exports of `utils/validation`. Tools and tool calls cross as JSON text, so a TypeBox schema reads as its JSON Schema.

use pi_ai::utils::validation;
use pi_ai::{Tool, ToolCall};
use wasm_bindgen::prelude::*;

use crate::json::{from_js_text, value_to_js};
use crate::options::throw;

/// `validateToolCall(tools, toolCall)`.
#[wasm_bindgen(js_name = aiValidateToolCall)]
pub fn validate_tool_call(tools: JsValue, tool_call: JsValue) -> Result<JsValue, JsValue> {
	let tools: Vec<Tool> = throw(from_js_text(&tools))?;
	let tool_call: ToolCall = throw(from_js_text(&tool_call))?;
	throw(validation::validate_tool_call(&tools, &tool_call).and_then(|value| value_to_js(&value)))
}

/// `validateToolArguments(tool, toolCall)`.
#[wasm_bindgen(js_name = aiValidateToolArguments)]
pub fn validate_tool_arguments(tool: JsValue, tool_call: JsValue) -> Result<JsValue, JsValue> {
	let tool: Tool = throw(from_js_text(&tool))?;
	let tool_call: ToolCall = throw(from_js_text(&tool_call))?;
	throw(validation::validate_tool_arguments(&tool, &tool_call).and_then(|value| value_to_js(&value)))
}
