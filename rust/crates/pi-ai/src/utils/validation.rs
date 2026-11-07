//! Port of `packages/ai/src/utils/validation.ts`.
//! PORT-STATUS: reviewed
//!
//! The TS cache holds one compiled validator per schema object in a `WeakMap`. A `serde_json::Value` has no
//! identity, so the Rust cache is keyed by the schema's JSON (`pi_host::js::json_stringify`) and keeps the most
//! recently compiled schemas, up to `VALIDATOR_CACHE_LIMIT`, per thread. A schema changed after its first use
//! therefore gets a validator of its own, where TS keeps validating with the one compiled first. The functions
//! return `PiResult<Value>`: the TS `throw`s (unknown tool, malformed arguments, failed validation, a schema that
//! does not compile) are `Err`s with the same messages.

use std::cell::RefCell;
use std::rc::Rc;

use indexmap::IndexMap;
use pi_host::js::{json_stringify, json_stringify_pretty};
use pi_host::{PiError, PiResult};
use pi_schema::formats::formats;
use pi_schema::{CoercingValidator, CompileOptions, ParseResult, UnresolvedRefs, compile_coercing};
use serde_json::Value;

use crate::types::{Tool, ToolCall};

const VALIDATOR_CACHE_LIMIT: usize = 256;

thread_local! {
	// One compiled validator per schema: tool schemas are long-lived and repeat across calls.
	static VALIDATOR_CACHE: RefCell<IndexMap<String, Rc<CoercingValidator>>> = RefCell::new(IndexMap::new());
}

fn get_validator(schema: &Value) -> PiResult<Rc<CoercingValidator>> {
	let key = json_stringify(schema);
	if let Some(cached) = VALIDATOR_CACHE.with(|cache| cache.borrow().get(&key).cloned()) {
		return Ok(cached);
	}
	// A $ref that names nothing (a dangling definition in an MCP server's schema) rejects only values at its place,
	// instead of making every call of the tool fail.
	let validator = Rc::new(compile_coercing(
		schema,
		&CompileOptions {
			formats: Some(formats()),
			coerce: Some(true),
			unresolved_refs: Some(UnresolvedRefs::Reject),
			..CompileOptions::default()
		},
	)?);
	VALIDATOR_CACHE.with(|cache| {
		let mut cache = cache.borrow_mut();
		if cache.len() >= VALIDATOR_CACHE_LIMIT {
			cache.shift_remove_index(0);
		}
		cache.insert(key, validator.clone());
	});
	Ok(validator)
}

/// A pi-schema error path (a JSON Pointer) as the dotted path the model sees: `/edits/0/oldText` -> `edits.0.oldText`.
fn format_validation_path(pointer: &str) -> String {
	if pointer.is_empty() {
		return "root".to_string();
	}
	pointer[1..]
		.split('/')
		.map(|segment| segment.replace("~1", "/").replace("~0", "~"))
		.collect::<Vec<_>>()
		.join(".")
}

/// Finds a tool by name and validates the tool call arguments against its JSON Schema
/// - `tools`: Array of tool definitions
/// - `tool_call`: The tool call from the LLM
///
/// Returns the validated arguments, or an error if the tool is not found or validation fails.
pub fn validate_tool_call(tools: &[Tool], tool_call: &ToolCall) -> PiResult<Value> {
	let Some(tool) = tools.iter().find(|t| t.name == tool_call.name) else {
		return Err(PiError::new(format!("Tool \"{}\" not found", tool_call.name)));
	};
	validate_tool_arguments(tool, tool_call)
}

/// Validates tool call arguments against the tool's JSON Schema (any JSON Schema: pi-schema's builders, TypeBox, plain
/// objects). The arguments are coerced first (pi-schema's `parse()`): models often send `"42"` for a number, a single
/// value for an array, or `null` for an omitted optional property.
/// - `tool`: The tool definition with its JSON Schema
/// - `tool_call`: The tool call from the LLM
///
/// Returns the validated (and potentially coerced) arguments, or an error with a formatted message if validation
/// fails.
pub fn validate_tool_arguments(tool: &Tool, tool_call: &ToolCall) -> PiResult<Value> {
	if let Some(arguments_error) = tool_call.arguments_error.as_deref().filter(|error| !error.is_empty()) {
		return Err(PiError::new(format!(
			"Invalid arguments for tool \"{}\": {arguments_error}",
			tool_call.name
		)));
	}
	// A copy, so tools that modify their arguments never modify the tool call in the transcript.
	let errors = match get_validator(&tool.parameters)?.parse(&Value::Object(tool_call.arguments.clone()))? {
		ParseResult::Ok(value) => return Ok(value),
		ParseResult::Err(errors) => errors,
	};

	let errors = errors
		.iter()
		.map(|error| format!("  - {}: {}", format_validation_path(&error.path), error.message))
		.collect::<Vec<_>>()
		.join("\n");

	let error_message = format!(
		"Validation failed for tool \"{}\":\n{errors}\n\nReceived arguments:\n{}",
		tool_call.name,
		json_stringify_pretty(&tool_call.arguments, 2)
	);

	Err(PiError::new(error_message))
}
