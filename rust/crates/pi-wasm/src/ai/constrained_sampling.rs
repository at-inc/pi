//! Exports of `api/constrained-sampling`.

use js_sys::{Function, Reflect};
use pi_ai::Tool;
use pi_ai::api::constrained_sampling::{self, GrammarToolInputJsonBuffer, UnsupportedStrictSchemaKeywordCheck};
use pi_host::js::json_stringify;
use serde_json::{Map, Value};
use wasm_bindgen::JsCast;
use wasm_bindgen::prelude::*;

use crate::options::{arg, throw};

/// `makeStrictJsonSchema(schema, isUnsupportedKeyword)`, through `JSON.parse`, which keeps a property named
/// `__proto__` an own property as the TS `structuredClone` does.
#[wasm_bindgen(js_name = aiMakeStrictJsonSchema)]
pub fn make_strict_json_schema(schema: JsValue, is_unsupported_keyword: Option<Function>) -> Result<JsValue, JsValue> {
	let schema: Value = arg(schema)?;
	let check = is_unsupported_keyword.map(keyword_check);
	let strict = throw(constrained_sampling::make_strict_json_schema(&schema, check.as_deref()))?;
	js_sys::JSON::parse(&json_stringify(&strict))
}

/// `prefersJsonSchemaStrictSampling(tool)`.
#[wasm_bindgen(js_name = aiPrefersJsonSchemaStrictSampling)]
pub fn prefers_json_schema_strict_sampling(tool: JsValue) -> Result<bool, JsValue> {
	Ok(constrained_sampling::prefers_json_schema_strict_sampling(&arg::<Tool>(
		tool,
	)?))
}

/// `isStrictToolLimitErrorMessage(message)`.
#[wasm_bindgen(js_name = aiIsStrictToolLimitErrorMessage)]
pub fn is_strict_tool_limit_error_message(message: &str) -> bool {
	constrained_sampling::is_strict_tool_limit_error_message(message)
}

/// `getJsonSchemaToolParameters(tool, strict)`: the tool's own `parameters` object when not strict, as in the TS.
#[wasm_bindgen(js_name = aiGetJsonSchemaToolParameters)]
pub fn get_json_schema_tool_parameters(tool: JsValue, strict: Option<bool>) -> Result<JsValue, JsValue> {
	if strict != Some(true) {
		return Reflect::get(&tool, &"parameters".into());
	}
	let parameters = throw(constrained_sampling::get_json_schema_tool_parameters(
		&arg::<Tool>(tool)?,
		strict,
	))?;
	js_sys::JSON::parse(&json_stringify(&parameters))
}

/// `getGrammarToolInput(toolName, arguments, inputProperty)`.
#[wasm_bindgen(js_name = aiGetGrammarToolInput)]
pub fn get_grammar_tool_input(tool_name: &str, arguments: JsValue, input_property: &str) -> Result<String, JsValue> {
	let arguments: Map<String, Value> = arg(arguments)?;
	throw(constrained_sampling::get_grammar_tool_input(
		tool_name,
		&arguments,
		input_property,
	))
}

/// `appendGrammarToolInputJsonDelta(buffer, inputProperty, inputDelta)`: updates the JS `buffer` in place.
#[wasm_bindgen(js_name = aiAppendGrammarToolInputJsonDelta)]
pub fn append_grammar_tool_input_json_delta(
	buffer: JsValue,
	input_property: &str,
	input_delta: &str,
) -> Result<Option<String>, JsValue> {
	let mut rust_buffer = read_buffer(&buffer);
	let piece =
		constrained_sampling::append_grammar_tool_input_json_delta(&mut rust_buffer, input_property, input_delta);
	write_buffer(&buffer, &rust_buffer);
	throw(piece)
}

/// `closeGrammarToolInputJson(buffer, inputProperty, finalInput)`: updates the JS `buffer` in place.
#[wasm_bindgen(js_name = aiCloseGrammarToolInputJson)]
pub fn close_grammar_tool_input_json(
	buffer: JsValue,
	input_property: &str,
	final_input: Option<String>,
) -> Result<Option<String>, JsValue> {
	let mut rust_buffer = read_buffer(&buffer);
	let rest =
		constrained_sampling::close_grammar_tool_input_json(&mut rust_buffer, input_property, final_input.as_deref());
	write_buffer(&buffer, &rust_buffer);
	throw(rest)
}

/// `resolveJsonSchemaStrictSampling(tool, supportsStrictMode, isUnsupportedKeyword)`.
#[wasm_bindgen(js_name = aiResolveJsonSchemaStrictSampling)]
pub fn resolve_json_schema_strict_sampling(
	tool: JsValue,
	supports_strict_mode: bool,
	is_unsupported_keyword: Option<Function>,
) -> Result<Option<bool>, JsValue> {
	let check = is_unsupported_keyword.map(keyword_check);
	throw(constrained_sampling::resolve_json_schema_strict_sampling(
		&arg::<Tool>(tool)?,
		supports_strict_mode,
		check.as_deref(),
	))
}

/// `resolveGrammarConstrainedSampling(tool, supportsOpenAIGrammarTools)`: `{ format, definition, inputProperty }` or
/// `undefined`.
#[wasm_bindgen(js_name = aiResolveGrammarConstrainedSampling)]
pub fn resolve_grammar_constrained_sampling(
	tool: JsValue,
	supports_openai_grammar_tools: bool,
) -> Result<JsValue, JsValue> {
	let grammar = throw(constrained_sampling::resolve_grammar_constrained_sampling(
		&arg::<Tool>(tool)?,
		supports_openai_grammar_tools,
	))?;
	let Some(grammar) = grammar else {
		return Ok(JsValue::UNDEFINED);
	};
	throw(crate::json::to_js(&serde_json::json!({
		"format": grammar.format.as_str(),
		"definition": grammar.definition,
		"inputProperty": grammar.input_property,
	})))
}

/// `createGrammarToolInputProperties(tools, supportsOpenAIGrammarTools)` as `[toolName, inputProperty]` entries, for
/// the facade's `Map`.
#[wasm_bindgen(js_name = aiCreateGrammarToolInputProperties)]
pub fn create_grammar_tool_input_properties(
	tools: JsValue,
	supports_openai_grammar_tools: bool,
) -> Result<JsValue, JsValue> {
	let tools: Option<Vec<Tool>> = arg(tools)?;
	let properties = throw(constrained_sampling::create_grammar_tool_input_properties(
		tools.as_deref(),
		supports_openai_grammar_tools,
	))?;
	let entries: Vec<(String, String)> = properties.into_iter().collect();
	throw(crate::json::to_js(&entries))
}

/// A JS `isUnsupportedKeyword(key, value)` as the Rust check; a throw rejects the conversion, as in the TS.
fn keyword_check(function: Function) -> Box<UnsupportedStrictSchemaKeywordCheck> {
	Box::new(move |key, value| {
		let value = crate::json::to_js(value)?;
		let result = function
			.call2(&JsValue::UNDEFINED, &key.into(), &value)
			.map_err(crate::error::from_js)?;
		Ok(result.is_truthy())
	})
}

/// The fields of a JS `GrammarToolInputJsonBuffer`.
fn read_buffer(buffer: &JsValue) -> GrammarToolInputJsonBuffer {
	let field = |name: &str| Reflect::get(buffer, &name.into()).unwrap_or(JsValue::UNDEFINED);
	GrammarToolInputJsonBuffer {
		input: field("input").as_string().unwrap_or_default(),
		started: field("started").is_truthy(),
		closed: field("closed").is_truthy(),
	}
}

/// Writes the fields of `rust_buffer` back to the JS `buffer`.
fn write_buffer(buffer: &JsValue, rust_buffer: &GrammarToolInputJsonBuffer) {
	let object = buffer.unchecked_ref::<js_sys::Object>();
	let _ = Reflect::set(object, &"input".into(), &rust_buffer.input.as_str().into());
	let _ = Reflect::set(object, &"started".into(), &rust_buffer.started.into());
	let _ = Reflect::set(object, &"closed".into(), &rust_buffer.closed.into());
}
