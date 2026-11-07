//! Port of `packages/ai/src/api/constrained-sampling.ts`.
//! PORT-STATUS: pending

// PORT-STUB: w3-bedrock-misc
// The items below are implemented by w3-openai-completions and w3-anthropic, which call them on every request; the
// owning shard replaces this block with the full port of the file.

use indexmap::IndexMap;
use pi_host::js::{js_key_order, json_stringify};
use pi_host::{PiError, PiResult};
use serde_json::{Map, Value, json};

use crate::types::{
	ConstrainedSamplingConfig, ConstrainedSamplingStrictness, GrammarFormat, JsonObject, Tool, ToolConstrainedSampling,
};
use crate::utils::text::js_trim;

/// `UnsupportedStrictJsonSchemaError`, or another error thrown while checking a schema.
enum StrictSchemaError {
	Unsupported(String),
	Other(PiError),
}

/// Returns true when a provider's strict mode rejects this schema keyword with this value.
pub type UnsupportedStrictSchemaKeywordCheck = dyn Fn(&str, &Value) -> PiResult<bool>;

const UNSUPPORTED_STRICT_SCHEMA_KEYS: [&str; 16] = [
	"$ref",
	"$defs",
	"definitions",
	"allOf",
	"oneOf",
	"patternProperties",
	"dependentSchemas",
	"dependencies",
	"unevaluatedProperties",
	"propertyNames",
	"contains",
	"prefixItems",
	"not",
	"if",
	"then",
	"else",
];

fn unsupported(message: impl Into<String>) -> StrictSchemaError {
	StrictSchemaError::Unsupported(message.into())
}

fn is_structured_schema(schema: &Value) -> bool {
	let Some(schema) = schema.as_object() else {
		return false;
	};
	let types: Vec<&Value> = match schema.get("type") {
		Some(Value::String(_)) => vec![&schema["type"]],
		Some(Value::Array(types)) => types.iter().collect(),
		_ => Vec::new(),
	};
	types.iter().any(|value| value.as_str() == Some("object"))
		|| types.iter().any(|value| value.as_str() == Some("array"))
		|| schema.contains_key("properties")
		|| schema.contains_key("items")
}

fn schema_allows_null(schema: &Value) -> bool {
	let Some(schema) = schema.as_object() else {
		return false;
	};
	match schema.get("type") {
		Some(Value::String(value)) if value == "null" => return true,
		Some(Value::Array(types)) if types.iter().any(|value| value.as_str() == Some("null")) => return true,
		_ => {}
	}
	if schema.get("const") == Some(&Value::Null)
		|| schema
			.get("enum")
			.and_then(Value::as_array)
			.is_some_and(|values| values.contains(&Value::Null))
	{
		return true;
	}
	schema
		.get("anyOf")
		.and_then(Value::as_array)
		.is_some_and(|variants| variants.iter().any(schema_allows_null))
}

/// `Object.keys(object)`: array-index keys first, ascending, then the others in insertion order.
fn js_keys(object: &Map<String, Value>) -> Vec<String> {
	let mut keys = Value::Object(object.keys().map(|key| (key.clone(), Value::Null)).collect());
	js_key_order(&mut keys);
	match keys {
		Value::Object(keys) => keys.into_iter().map(|(key, _)| key).collect(),
		_ => unreachable!("an object stays an object"),
	}
}

fn make_json_schema_node_strict(
	schema: &mut Value,
	is_unsupported_keyword: Option<&UnsupportedStrictSchemaKeywordCheck>,
) -> Result<(), StrictSchemaError> {
	let Some(object) = schema.as_object_mut() else {
		return Err(unsupported("boolean schemas are unsupported"));
	};
	for key in UNSUPPORTED_STRICT_SCHEMA_KEYS {
		if object.contains_key(key) {
			return Err(unsupported(format!("{key} schemas are unsupported")));
		}
	}
	if let Some(is_unsupported_keyword) = is_unsupported_keyword {
		for key in js_keys(object) {
			let value = &object[&key];
			if is_unsupported_keyword(&key, value).map_err(StrictSchemaError::Other)? {
				return Err(unsupported(format!("{key}: {} is unsupported", json_stringify(value))));
			}
		}
	}

	if let Some(any_of) = object.get_mut("anyOf") {
		let Some(variants) = any_of.as_array_mut().filter(|variants| !variants.is_empty()) else {
			return Err(unsupported("anyOf must contain at least one schema"));
		};
		for variant in variants {
			if is_structured_schema(variant) {
				return Err(unsupported("object and array unions are unsupported"));
			}
			make_json_schema_node_strict(variant, is_unsupported_keyword)?;
		}
	}

	if let Some(items) = object.get_mut("items") {
		if items.is_array() {
			return Err(unsupported("tuple schemas are unsupported"));
		}
		make_json_schema_node_strict(items, is_unsupported_keyword)?;
	}

	let is_object_schema = object.get("type").and_then(Value::as_str) == Some("object");
	if object.contains_key("properties") && !is_object_schema {
		return Err(unsupported("properties require type object"));
	}
	if !is_object_schema {
		return Ok(());
	}
	if object
		.get("additionalProperties")
		.is_some_and(|value| *value != Value::Bool(false))
	{
		return Err(unsupported("schema-valued or true additionalProperties is unsupported"));
	}
	if object.get("properties").is_some_and(|value| !value.is_object()) {
		return Err(unsupported("object properties must be a schema map"));
	}
	if object.get("required").is_some_and(|required| {
		required
			.as_array()
			.is_none_or(|keys| keys.iter().any(|key| !key.is_string()))
	}) {
		return Err(unsupported("object required must be a string array"));
	}

	let required: Vec<String> = object
		.get("required")
		.and_then(Value::as_array)
		.map(|keys| keys.iter().filter_map(Value::as_str).map(str::to_string).collect())
		.unwrap_or_default();
	let property_names = object
		.get("properties")
		.and_then(Value::as_object)
		.map(js_keys)
		.unwrap_or_default();
	if required.iter().any(|key| !property_names.contains(key)) {
		return Err(unsupported("required contains an unknown property"));
	}
	if let Some(Value::Object(properties)) = object.get_mut("properties") {
		for key in &property_names {
			let property = properties.get_mut(key).expect("property names come from the map");
			make_json_schema_node_strict(property, is_unsupported_keyword)?;
			if !required.contains(key) && !schema_allows_null(property) {
				let property = std::mem::take(property);
				properties.insert(key.clone(), json!({ "anyOf": [property, { "type": "null" }] }));
			}
		}
	}
	object.insert(
		"required".into(),
		Value::Array(property_names.into_iter().map(Value::String).collect()),
	);
	object.insert("additionalProperties".into(), Value::Bool(false));
	Ok(())
}

fn make_strict_json_schema_inner(
	schema: &Value,
	is_unsupported_keyword: Option<&UnsupportedStrictSchemaKeywordCheck>,
) -> Result<Value, StrictSchemaError> {
	let mut cloned = schema.clone();
	if !cloned.is_object() {
		return Err(unsupported("root schema must have type object"));
	}
	make_json_schema_node_strict(&mut cloned, is_unsupported_keyword)?;
	if cloned.get("type").and_then(Value::as_str) != Some("object") {
		return Err(unsupported("root schema must have type object"));
	}
	Ok(cloned)
}

/// Convert a tool schema to the strict subset expected by provider constrained sampling.
pub fn make_strict_json_schema(
	schema: &Value,
	is_unsupported_keyword: Option<&UnsupportedStrictSchemaKeywordCheck>,
) -> PiResult<Value> {
	make_strict_json_schema_inner(schema, is_unsupported_keyword).map_err(|error| match error {
		StrictSchemaError::Unsupported(message) => PiError::new(message),
		StrictSchemaError::Other(error) => error,
	})
}

/// Returns true for a tool that asks for strict JSON-schema sampling only when possible (`strict: "prefer"`).
pub fn prefers_json_schema_strict_sampling(tool: &Tool) -> bool {
	matches!(
		tool.constrained_sampling,
		Some(ToolConstrainedSampling::Config(ConstrainedSamplingConfig::JsonSchema {
			strict: ConstrainedSamplingStrictness::Prefer
		}))
	)
}

/// Claude rejects (on the Anthropic API and on Bedrock) a request whose strict tools exceed its limits with one of these
/// messages. Observed limits: 20 strict tools, and 16 optional or union-typed parameters across all strict tools.
const STRICT_TOOL_LIMIT_ERROR_MESSAGES: [&str; 3] = [
	"The compiled grammar is too large",
	"Too many strict tools (",
	"Schemas contains too many parameters with union types (",
];

/// Returns true when a provider error message says the request's strict tools exceed the provider's limits.
pub fn is_strict_tool_limit_error_message(message: &str) -> bool {
	STRICT_TOOL_LIMIT_ERROR_MESSAGES
		.iter()
		.any(|limit_message| message.contains(limit_message))
}

pub fn get_json_schema_tool_parameters(tool: &Tool, strict: Option<bool>) -> PiResult<Value> {
	if strict == Some(true) {
		make_strict_json_schema(&tool.parameters, None)
	} else {
		Ok(tool.parameters.clone())
	}
}

/// `GrammarConstrainedSampling.format`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GrammarSyntax {
	Lark,
	Regex,
}

impl GrammarSyntax {
	/// The wire value.
	pub fn as_str(self) -> &'static str {
		match self {
			GrammarSyntax::Lark => "lark",
			GrammarSyntax::Regex => "regex",
		}
	}
}

#[derive(Clone, Debug, PartialEq)]
pub struct GrammarConstrainedSampling {
	pub format: GrammarSyntax,
	pub definition: String,
	pub input_property: String,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct GrammarToolInputJsonBuffer {
	pub input: String,
	pub started: bool,
	pub closed: bool,
}

pub fn get_grammar_tool_input(tool_name: &str, arguments: &JsonObject, input_property: &str) -> PiResult<String> {
	match arguments.get(input_property) {
		Some(Value::String(input)) => Ok(input.clone()),
		_ => Err(PiError::new(format!(
			"Grammar tool call \"{tool_name}\" requires argument \"{input_property}\" to be a string."
		))),
	}
}

/// `JSON.stringify(text).slice(1, -1)`.
fn json_string_body(text: &str) -> String {
	let quoted = json_stringify(text);
	quoted[1..quoted.len() - 1].to_string()
}

/// Append `inputDelta` to a streaming grammar tool input and return the matching piece of its JSON arguments text, or
/// `undefined` for an empty delta. Costs time linear in the delta, never in the input so far.
pub fn append_grammar_tool_input_json_delta(
	buffer: &mut GrammarToolInputJsonBuffer,
	input_property: &str,
	input_delta: &str,
) -> PiResult<Option<String>> {
	if buffer.closed {
		return Err(PiError::new(format!(
			"grammar tool input for property \"{input_property}\" changed after it was closed"
		)));
	}
	if input_delta.is_empty() {
		return Ok(None);
	}
	buffer.input.push_str(input_delta);
	Ok(Some(format!(
		"{}{}",
		grammar_tool_input_json_start(buffer, input_property),
		json_string_body(input_delta)
	)))
}

/// Close a streaming grammar tool input and return the rest of its JSON arguments text. `finalInput` is the complete
/// input when the provider sends one; it must continue the input streamed so far. Closing again with the same input
/// returns `undefined`.
pub fn close_grammar_tool_input_json(
	buffer: &mut GrammarToolInputJsonBuffer,
	input_property: &str,
	final_input: Option<&str>,
) -> PiResult<Option<String>> {
	if buffer.closed {
		if final_input.is_none_or(|final_input| final_input == buffer.input) {
			return Ok(None);
		}
		return Err(PiError::new(format!(
			"grammar tool input for property \"{input_property}\" changed after it was closed"
		)));
	}
	if let Some(final_input) = final_input
		&& !final_input.starts_with(buffer.input.as_str())
	{
		return Err(PiError::new(format!(
			"grammar tool input for property \"{input_property}\" changed non-monotonically"
		)));
	}
	let rest = final_input.map_or(String::new(), |final_input| {
		final_input[buffer.input.len()..].to_string()
	});
	if let Some(final_input) = final_input {
		buffer.input = final_input.to_string();
	}
	buffer.closed = true;
	Ok(Some(format!(
		"{}{}\"}}",
		grammar_tool_input_json_start(buffer, input_property),
		json_string_body(&rest)
	)))
}

/// The opening of the JSON arguments text, once.
fn grammar_tool_input_json_start(buffer: &mut GrammarToolInputJsonBuffer, input_property: &str) -> String {
	if buffer.started {
		return String::new();
	}
	buffer.started = true;
	format!("{{{}:\"", json_stringify(input_property))
}

fn infer_grammar_input_property(tool: &Tool) -> Result<String, String> {
	let schema = &tool.parameters;
	if schema.get("type").and_then(Value::as_str) != Some("object") {
		return Err("grammar constrained sampling requires an object parameter schema".into());
	}
	let input_property = match schema.get("required").and_then(Value::as_array).map(Vec::as_slice) {
		Some([Value::String(input_property)]) => input_property.clone(),
		_ => return Err("grammar constrained sampling requires exactly one required string property".into()),
	};
	let property = schema
		.get("properties")
		.and_then(|properties| properties.get(&input_property));
	if !property.is_some_and(is_truthy) {
		return Err(format!(
			"grammar constrained sampling requires a properties entry for {input_property}"
		));
	}
	if property
		.and_then(|property| property.get("type"))
		.and_then(Value::as_str)
		!= Some("string")
	{
		return Err(format!(
			"grammar constrained sampling property {input_property} must have type string"
		));
	}
	Ok(input_property)
}

/// JS truthiness of a JSON value.
fn is_truthy(value: &Value) -> bool {
	match value {
		Value::Null => false,
		Value::Bool(value) => *value,
		Value::Number(number) => number.as_f64().is_some_and(|number| number != 0.0 && !number.is_nan()),
		Value::String(value) => !value.is_empty(),
		Value::Array(_) | Value::Object(_) => true,
	}
}

/// Decide whether a JSON-schema tool is sent in strict mode. `isUnsupportedKeyword` lets a provider
/// reject extra keywords its strict mode does not accept, so "prefer" tools fall back to non-strict.
pub fn resolve_json_schema_strict_sampling(
	tool: &Tool,
	supports_strict_mode: bool,
	is_unsupported_keyword: Option<&UnsupportedStrictSchemaKeywordCheck>,
) -> PiResult<Option<bool>> {
	let Some(ToolConstrainedSampling::Config(ConstrainedSamplingConfig::JsonSchema { strict })) =
		&tool.constrained_sampling
	else {
		return Ok(None);
	};

	if supports_strict_mode {
		return match make_strict_json_schema_inner(&tool.parameters, is_unsupported_keyword) {
			Ok(_) => Ok(Some(true)),
			Err(StrictSchemaError::Other(error)) => Err(error),
			Err(StrictSchemaError::Unsupported(_)) if *strict != ConstrainedSamplingStrictness::Require => Ok(None),
			Err(StrictSchemaError::Unsupported(message)) => Err(PiError::new(format!(
				"Tool \"{}\" requires JSON-schema constrained sampling, but {message}.",
				tool.name
			))),
		};
	}
	if *strict == ConstrainedSamplingStrictness::Require {
		return Err(PiError::new(format!(
			"Tool \"{}\" requires JSON-schema constrained sampling, but strict tools are unsupported.",
			tool.name
		)));
	}
	Ok(None)
}

pub fn resolve_grammar_constrained_sampling(
	tool: &Tool,
	supports_openai_grammar_tools: bool,
) -> PiResult<Option<GrammarConstrainedSampling>> {
	let Some(ToolConstrainedSampling::Config(ConstrainedSamplingConfig::Grammar { variants })) =
		&tool.constrained_sampling
	else {
		return Ok(None);
	};

	if !supports_openai_grammar_tools {
		return Ok(None);
	}

	let lark_definition = variants.get(&GrammarFormat::OpenaiLark);
	let regex_definition = variants.get(&GrammarFormat::OpenaiRegex);
	let has_lark_definition = lark_definition.is_some_and(|definition| !js_trim(definition).is_empty());
	let has_regex_definition = regex_definition.is_some_and(|definition| !js_trim(definition).is_empty());
	if !has_lark_definition && !has_regex_definition {
		return Err(PiError::new(format!(
			"Tool \"{}\" cannot use grammar constrained sampling: no supported grammar variant was provided.",
			tool.name
		)));
	}

	match infer_grammar_input_property(tool) {
		Ok(input_property) => Ok(Some(GrammarConstrainedSampling {
			format: if has_lark_definition {
				GrammarSyntax::Lark
			} else {
				GrammarSyntax::Regex
			},
			definition: if has_lark_definition {
				lark_definition
			} else {
				regex_definition
			}
			.cloned()
			.expect("a present grammar definition"),
			input_property,
		})),
		Err(message) => Err(PiError::new(format!(
			"Tool \"{}\" cannot use grammar constrained sampling: {message}.",
			tool.name
		))),
	}
}

pub fn create_grammar_tool_input_properties(
	tools: Option<&[Tool]>,
	supports_openai_grammar_tools: bool,
) -> PiResult<IndexMap<String, String>> {
	let mut properties = IndexMap::new();
	for tool in tools.unwrap_or_default() {
		if let Some(grammar) = resolve_grammar_constrained_sampling(tool, supports_openai_grammar_tools)? {
			properties.insert(tool.name.clone(), grammar.input_property);
		}
	}
	Ok(properties)
}
