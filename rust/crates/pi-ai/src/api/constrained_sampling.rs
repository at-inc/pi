//! Port of `packages/ai/src/api/constrained-sampling.ts`.
//! PORT-STATUS: ported
//!
//! The TS `UnsupportedStrictJsonSchemaError` (a plain `Error` subclass, `name` `"Error"`) is the internal
//! `UnsupportedStrictJsonSchema` marker, so `resolve_json_schema_strict_sampling` can tell it from other errors; it
//! leaves this module as `PiError::new(message)`. An `UnsupportedStrictSchemaKeywordCheck` cannot throw. Schema
//! objects are walked in JS property order (array-index keys first).

use indexmap::IndexMap;
use pi_host::js::json_stringify;
use pi_host::{PiError, PiResult};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

use crate::types::{
	ConstrainedSamplingConfig, ConstrainedSamplingStrictness, GrammarFormat, Tool, ToolConstrainedSampling,
};
use crate::utils::text::{js_entries, js_trim};

/// `UnsupportedStrictJsonSchemaError`.
struct UnsupportedStrictJsonSchema(String);

fn unsupported(message: impl Into<String>) -> UnsupportedStrictJsonSchema {
	UnsupportedStrictJsonSchema(message.into())
}

/// Returns true when a provider's strict mode rejects this schema keyword with this value.
pub type UnsupportedStrictSchemaKeywordCheck = dyn Fn(&str, &Value) -> bool;

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

/// `Object.keys(map)`: JS property order.
fn js_keys(map: &Map<String, Value>) -> Vec<String> {
	let keys: IndexMap<String, ()> = map.keys().map(|key| (key.clone(), ())).collect();
	js_entries(&keys).into_iter().map(|(key, ())| key.clone()).collect()
}

fn is_structured_schema(schema: &Value) -> bool {
	let Value::Object(schema) = schema else {
		return false;
	};
	let has_type = |name: &str| match schema.get("type") {
		Some(Value::String(r#type)) => r#type == name,
		Some(Value::Array(types)) => types.iter().any(|r#type| r#type.as_str() == Some(name)),
		_ => false,
	};
	has_type("object") || has_type("array") || schema.contains_key("properties") || schema.contains_key("items")
}

fn schema_allows_null(schema: &Value) -> bool {
	let Value::Object(schema) = schema else {
		return false;
	};
	match schema.get("type") {
		Some(Value::String(r#type)) if r#type == "null" => return true,
		Some(Value::Array(types)) if types.iter().any(|r#type| r#type.as_str() == Some("null")) => return true,
		_ => {}
	}
	if schema.get("const") == Some(&Value::Null)
		|| matches!(schema.get("enum"), Some(Value::Array(values)) if values.contains(&Value::Null))
	{
		return true;
	}
	matches!(schema.get("anyOf"), Some(Value::Array(variants)) if variants.iter().any(schema_allows_null))
}

fn make_json_schema_node_strict(
	schema: &mut Value,
	is_unsupported_keyword: Option<&UnsupportedStrictSchemaKeywordCheck>,
) -> Result<(), UnsupportedStrictJsonSchema> {
	let Value::Object(schema) = schema else {
		return Err(unsupported("boolean schemas are unsupported"));
	};
	for key in UNSUPPORTED_STRICT_SCHEMA_KEYS {
		if schema.contains_key(key) {
			return Err(unsupported(format!("{key} schemas are unsupported")));
		}
	}
	if let Some(is_unsupported_keyword) = is_unsupported_keyword {
		for key in js_keys(schema) {
			let value = &schema[&key];
			if is_unsupported_keyword(&key, value) {
				return Err(unsupported(format!("{key}: {} is unsupported", json_stringify(value))));
			}
		}
	}

	if let Some(any_of) = schema.get_mut("anyOf") {
		let Value::Array(variants) = any_of else {
			return Err(unsupported("anyOf must contain at least one schema"));
		};
		if variants.is_empty() {
			return Err(unsupported("anyOf must contain at least one schema"));
		}
		for variant in variants {
			if is_structured_schema(variant) {
				return Err(unsupported("object and array unions are unsupported"));
			}
			make_json_schema_node_strict(variant, is_unsupported_keyword)?;
		}
	}

	if let Some(items) = schema.get_mut("items") {
		if items.is_array() {
			return Err(unsupported("tuple schemas are unsupported"));
		}
		make_json_schema_node_strict(items, is_unsupported_keyword)?;
	}

	let is_object_schema = schema.get("type").and_then(Value::as_str) == Some("object");
	if schema.contains_key("properties") && !is_object_schema {
		return Err(unsupported("properties require type object"));
	}
	if !is_object_schema {
		return Ok(());
	}
	if schema
		.get("additionalProperties")
		.is_some_and(|value| *value != Value::Bool(false))
	{
		return Err(unsupported("schema-valued or true additionalProperties is unsupported"));
	}
	if schema.get("properties").is_some_and(|value| !value.is_object()) {
		return Err(unsupported("object properties must be a schema map"));
	}
	let required: Vec<String> = match schema.get("required") {
		None => Vec::new(),
		Some(Value::Array(keys)) => keys
			.iter()
			.map(|key| key.as_str().map(str::to_string))
			.collect::<Option<_>>()
			.ok_or_else(|| unsupported("object required must be a string array"))?,
		Some(_) => return Err(unsupported("object required must be a string array")),
	};

	let mut detached = Map::new();
	let properties = match schema.get_mut("properties") {
		Some(Value::Object(properties)) => properties,
		_ => &mut detached,
	};
	let property_names = js_keys(properties);
	if required.iter().any(|key| !property_names.contains(key)) {
		return Err(unsupported("required contains an unknown property"));
	}
	for key in &property_names {
		let property = properties.get_mut(key).expect("property names come from the map");
		make_json_schema_node_strict(property, is_unsupported_keyword)?;
		if !required.contains(key) && !schema_allows_null(property) {
			let original = property.take();
			*property = json!({ "anyOf": [original, { "type": "null" }] });
		}
	}
	schema.insert(
		"required".into(),
		Value::Array(property_names.into_iter().map(Value::String).collect()),
	);
	schema.insert("additionalProperties".into(), Value::Bool(false));
	Ok(())
}

fn strict_json_schema(
	schema: &Value,
	is_unsupported_keyword: Option<&UnsupportedStrictSchemaKeywordCheck>,
) -> Result<Map<String, Value>, UnsupportedStrictJsonSchema> {
	let mut cloned = schema.clone();
	if !cloned.is_object() {
		return Err(unsupported("root schema must have type object"));
	}
	make_json_schema_node_strict(&mut cloned, is_unsupported_keyword)?;
	let Value::Object(cloned) = cloned else {
		unreachable!("the root schema was checked to be an object");
	};
	if cloned.get("type").and_then(Value::as_str) != Some("object") {
		return Err(unsupported("root schema must have type object"));
	}
	Ok(cloned)
}

/// Convert a tool schema to the strict subset expected by provider constrained sampling.
pub fn make_strict_json_schema(
	schema: &Value,
	is_unsupported_keyword: Option<&UnsupportedStrictSchemaKeywordCheck>,
) -> PiResult<Map<String, Value>> {
	strict_json_schema(schema, is_unsupported_keyword)
		.map_err(|UnsupportedStrictJsonSchema(message)| PiError::new(message))
}

/// `tool.constrainedSampling` when it is a config (not absent or `false`).
fn constrained_sampling_config(tool: &Tool) -> Option<&ConstrainedSamplingConfig> {
	match tool.constrained_sampling.as_ref()? {
		ToolConstrainedSampling::Config(config) => Some(config),
		ToolConstrainedSampling::Disabled(_) => None,
	}
}

/// Returns true for a tool that asks for strict JSON-schema sampling only when possible (`strict: "prefer"`).
pub fn prefers_json_schema_strict_sampling(tool: &Tool) -> bool {
	matches!(
		constrained_sampling_config(tool),
		Some(ConstrainedSamplingConfig::JsonSchema {
			strict: ConstrainedSamplingStrictness::Prefer
		})
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

/// `getJsonSchemaToolParameters(tool, strict)`.
pub fn get_json_schema_tool_parameters(tool: &Tool, strict: Option<bool>) -> PiResult<Value> {
	if strict == Some(true) {
		return make_strict_json_schema(&tool.parameters, None).map(Value::Object);
	}
	Ok(tool.parameters.clone())
}

/// `GrammarConstrainedSampling.format`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum GrammarSyntax {
	#[serde(rename = "lark")]
	Lark,
	#[serde(rename = "regex")]
	Regex,
}

impl GrammarSyntax {
	/// The TS string value.
	pub fn as_str(self) -> &'static str {
		match self {
			GrammarSyntax::Lark => "lark",
			GrammarSyntax::Regex => "regex",
		}
	}
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GrammarConstrainedSampling {
	pub format: GrammarSyntax,
	pub definition: String,
	pub input_property: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct GrammarToolInputJsonBuffer {
	pub input: String,
	pub started: bool,
	pub closed: bool,
}

/// `getGrammarToolInput(toolName, arguments, inputProperty)`: the string input of a grammar tool call.
pub fn get_grammar_tool_input(
	tool_name: &str,
	arguments: &Map<String, Value>,
	input_property: &str,
) -> PiResult<String> {
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

/// Append `input_delta` to a streaming grammar tool input and return the matching piece of its JSON arguments text, or
/// `None` for an empty delta. Costs time linear in the delta, never in the input so far.
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
	Ok(Some(
		grammar_tool_input_json_start(buffer, input_property) + &json_string_body(input_delta),
	))
}

/// Close a streaming grammar tool input and return the rest of its JSON arguments text. `final_input` is the complete
/// input when the provider sends one; it must continue the input streamed so far. Closing again with the same input
/// returns `None`.
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

/// JS truthiness of a JSON value.
fn truthy(value: Option<&Value>) -> bool {
	match value {
		None | Some(Value::Null) => false,
		Some(Value::Bool(value)) => *value,
		Some(Value::Number(number)) => number.as_f64().is_some_and(|number| number != 0.0),
		Some(Value::String(text)) => !text.is_empty(),
		Some(Value::Array(_) | Value::Object(_)) => true,
	}
}

fn infer_grammar_input_property(tool: &Tool) -> PiResult<String> {
	let schema = &tool.parameters;
	if schema.is_null() {
		return Err(PiError::type_error("Cannot read properties of null (reading 'type')"));
	}
	if schema.get("type").and_then(Value::as_str) != Some("object") {
		return Err(PiError::new(
			"grammar constrained sampling requires an object parameter schema",
		));
	}
	let input_property = match schema.get("required") {
		Some(Value::Array(required)) if required.len() == 1 => required[0].as_str(),
		_ => None,
	};
	let Some(input_property) = input_property else {
		return Err(PiError::new(
			"grammar constrained sampling requires exactly one required string property",
		));
	};

	let property = schema.get("properties").map_or(JsProperty::Missing, |properties| {
		js_property(properties, input_property)
	});
	let (present, string_type) = match property {
		JsProperty::Missing => (false, false),
		JsProperty::Value(property) => (
			truthy(Some(property)),
			property.get("type").and_then(Value::as_str) == Some("string"),
		),
		JsProperty::Opaque { truthy } => (truthy, false),
	};
	if !present {
		return Err(PiError::new(format!(
			"grammar constrained sampling requires a properties entry for {input_property}"
		)));
	}
	if !string_type {
		return Err(PiError::new(format!(
			"grammar constrained sampling property {input_property} must have type string"
		)));
	}
	Ok(input_property.to_string())
}

/// What `value[key]` reads in JS, as far as `inferGrammarInputProperty` looks at it.
enum JsProperty<'a> {
	Missing,
	Value(&'a Value),
	/// An inherited member, an array `length` or a string character: present when truthy, never with `type`.
	Opaque {
		truthy: bool,
	},
}

/// The members `k in Object.prototype` is true for.
const OBJECT_PROTOTYPE_MEMBERS: [&str; 12] = [
	"constructor",
	"__defineGetter__",
	"__defineSetter__",
	"hasOwnProperty",
	"__lookupGetter__",
	"__lookupSetter__",
	"isPrototypeOf",
	"propertyIsEnumerable",
	"toString",
	"valueOf",
	"__proto__",
	"toLocaleString",
];

/// `value?.[key]` for a JSON value, including inherited `Object.prototype` members and array and string indices.
/// `Array.prototype` and `String.prototype` methods are not modeled.
fn js_property<'a>(value: &'a Value, key: &str) -> JsProperty<'a> {
	let index = || key.parse::<usize>().ok().filter(|index| index.to_string() == key);
	match value {
		Value::Object(object) => match object.get(key) {
			Some(property) => JsProperty::Value(property),
			None if OBJECT_PROTOTYPE_MEMBERS.contains(&key) => JsProperty::Opaque { truthy: true },
			None => JsProperty::Missing,
		},
		Value::Array(array) if key == "length" => JsProperty::Opaque {
			truthy: !array.is_empty(),
		},
		Value::Array(array) => match index().and_then(|index| array.get(index)) {
			Some(property) => JsProperty::Value(property),
			None if OBJECT_PROTOTYPE_MEMBERS.contains(&key) => JsProperty::Opaque { truthy: true },
			None => JsProperty::Missing,
		},
		Value::String(text) if key == "length" => JsProperty::Opaque {
			truthy: !text.is_empty(),
		},
		Value::String(text) => match index() {
			Some(index) if index < pi_host::js::utf16_len(text) => JsProperty::Opaque { truthy: true },
			_ => JsProperty::Missing,
		},
		_ => JsProperty::Missing,
	}
}

/// Decide whether a JSON-schema tool is sent in strict mode. `is_unsupported_keyword` lets a provider
/// reject extra keywords its strict mode does not accept, so "prefer" tools fall back to non-strict.
pub fn resolve_json_schema_strict_sampling(
	tool: &Tool,
	supports_strict_mode: bool,
	is_unsupported_keyword: Option<&UnsupportedStrictSchemaKeywordCheck>,
) -> PiResult<Option<bool>> {
	let Some(ConstrainedSamplingConfig::JsonSchema { strict }) = constrained_sampling_config(tool) else {
		return Ok(None);
	};

	if supports_strict_mode {
		return match strict_json_schema(&tool.parameters, is_unsupported_keyword) {
			Ok(_) => Ok(Some(true)),
			Err(_) if *strict != ConstrainedSamplingStrictness::Require => Ok(None),
			Err(UnsupportedStrictJsonSchema(message)) => Err(PiError::new(format!(
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

/// `resolveGrammarConstrainedSampling(tool, supportsOpenAIGrammarTools)`.
pub fn resolve_grammar_constrained_sampling(
	tool: &Tool,
	supports_open_ai_grammar_tools: bool,
) -> PiResult<Option<GrammarConstrainedSampling>> {
	let Some(ConstrainedSamplingConfig::Grammar { variants }) = constrained_sampling_config(tool) else {
		return Ok(None);
	};

	if !supports_open_ai_grammar_tools {
		return Ok(None);
	}

	let definition = |format: GrammarFormat| {
		variants
			.get(&format)
			.filter(|definition| !js_trim(definition).is_empty())
	};
	let (format, definition) = match (
		definition(GrammarFormat::OpenaiLark),
		definition(GrammarFormat::OpenaiRegex),
	) {
		(Some(lark), _) => (GrammarSyntax::Lark, lark),
		(None, Some(regex)) => (GrammarSyntax::Regex, regex),
		(None, None) => {
			return Err(PiError::new(format!(
				"Tool \"{}\" cannot use grammar constrained sampling: no supported grammar variant was provided.",
				tool.name
			)));
		}
	};

	let input_property = infer_grammar_input_property(tool).map_err(|error| {
		PiError::new(format!(
			"Tool \"{}\" cannot use grammar constrained sampling: {}.",
			tool.name, error.message
		))
	})?;
	Ok(Some(GrammarConstrainedSampling {
		format,
		definition: definition.clone(),
		input_property,
	}))
}

/// `createGrammarToolInputProperties(tools, supportsOpenAIGrammarTools)`: grammar tool names mapped to their input
/// property.
pub fn create_grammar_tool_input_properties(
	tools: Option<&[Tool]>,
	supports_open_ai_grammar_tools: bool,
) -> PiResult<IndexMap<String, String>> {
	let mut properties = IndexMap::new();
	for tool in tools.unwrap_or_default() {
		if let Some(grammar) = resolve_grammar_constrained_sampling(tool, supports_open_ai_grammar_tools)? {
			properties.insert(tool.name.clone(), grammar.input_property);
		}
	}
	Ok(properties)
}
