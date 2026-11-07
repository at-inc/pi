//! Port of `packages/ai/src/api/constrained-sampling.ts`.
//! PORT-STATUS: pending

// PORT-STUB: w3-bedrock-misc
// Predicted by w3-openai-responses for `openai_responses_shared.rs`, `openai_responses.rs`,
// `azure_openai_responses.rs` and `openai_codex_responses.rs`.
use std::rc::Rc;

use indexmap::IndexMap;
use pi_host::PiResult;
use serde_json::Value;

use crate::types::{JsonObject, Tool};

/// `UnsupportedStrictSchemaKeywordCheck`.
pub type UnsupportedStrictSchemaKeywordCheck = Rc<dyn Fn(&str, &Value) -> bool>;

/// `GrammarConstrainedSampling`; `format` is `"lark"` or `"regex"`.
#[derive(Clone, Debug, PartialEq)]
pub struct GrammarConstrainedSampling {
	pub format: String,
	pub definition: String,
	pub input_property: String,
}

/// `GrammarToolInputJsonBuffer`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct GrammarToolInputJsonBuffer {
	pub input: String,
	pub started: bool,
	pub closed: bool,
}

pub fn get_json_schema_tool_parameters(_tool: &Tool, _strict: Option<bool>) -> PiResult<Value> {
	todo!("PORT-STUB")
}

pub fn get_grammar_tool_input(_tool_name: &str, _arguments: &JsonObject, _input_property: &str) -> PiResult<String> {
	todo!("PORT-STUB")
}

pub fn append_grammar_tool_input_json_delta(
	_buffer: &mut GrammarToolInputJsonBuffer,
	_input_property: &str,
	_input_delta: &str,
) -> PiResult<Option<String>> {
	todo!("PORT-STUB")
}

pub fn close_grammar_tool_input_json(
	_buffer: &mut GrammarToolInputJsonBuffer,
	_input_property: &str,
	_final_input: Option<&str>,
) -> PiResult<Option<String>> {
	todo!("PORT-STUB")
}

pub fn resolve_json_schema_strict_sampling(
	_tool: &Tool,
	_supports_strict_mode: bool,
	_is_unsupported_keyword: Option<&UnsupportedStrictSchemaKeywordCheck>,
) -> PiResult<Option<bool>> {
	todo!("PORT-STUB")
}

pub fn resolve_grammar_constrained_sampling(
	_tool: &Tool,
	_supports_openai_grammar_tools: bool,
) -> PiResult<Option<GrammarConstrainedSampling>> {
	todo!("PORT-STUB")
}

pub fn create_grammar_tool_input_properties(
	_tools: Option<&[Tool]>,
	_supports_openai_grammar_tools: bool,
) -> PiResult<IndexMap<String, String>> {
	todo!("PORT-STUB")
}
