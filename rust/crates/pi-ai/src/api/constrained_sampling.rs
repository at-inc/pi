//! Port of `packages/ai/src/api/constrained-sampling.ts`.
//! PORT-STATUS: pending

// PORT-STUB: w3-bedrock-misc
// The JSON-schema strict-sampling half of the file, which `api/anthropic_messages.rs` (w3-anthropic) calls. The bodies
// are ported from the TS instead of `todo!` so the Anthropic suites run; the owning shard replaces them with its port
// of the whole file. `UnsupportedStrictJsonSchemaError` is the private `StrictSchemaError`; a `structuredClone`d
// schema gets JS key order (`pi_host::js::js_key_order`) so `Object.keys`/`Object.entries` walks match.

use pi_host::{PiError, PiResult};
use serde_json::{Map, Value, json};

use crate::types::{ConstrainedSamplingConfig, ConstrainedSamplingStrictness, Tool, ToolConstrainedSampling};

/// `UnsupportedStrictJsonSchemaError`.
struct StrictSchemaError(String);

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

fn is_structured_schema(schema: &Value) -> bool {
	let Value::Object(schema) = schema else {
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
	let Value::Object(schema) = schema else {
		return false;
	};
	match schema.get("type") {
		Some(Value::String(value)) if value == "null" => return true,
		Some(Value::Array(types)) if types.iter().any(|value| value.as_str() == Some("null")) => return true,
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
) -> Result<(), StrictSchemaError> {
	let Value::Object(schema) = schema else {
		return Err(StrictSchemaError("boolean schemas are unsupported".into()));
	};
	for key in UNSUPPORTED_STRICT_SCHEMA_KEYS {
		if schema.contains_key(key) {
			return Err(StrictSchemaError(format!("{key} schemas are unsupported")));
		}
	}
	if let Some(is_unsupported_keyword) = is_unsupported_keyword {
		for (key, value) in schema.iter() {
			if is_unsupported_keyword(key, value) {
				return Err(StrictSchemaError(format!(
					"{key}: {} is unsupported",
					pi_host::js::json_stringify(value)
				)));
			}
		}
	}

	if let Some(any_of) = schema.get_mut("anyOf") {
		let Value::Array(variants) = any_of else {
			return Err(StrictSchemaError("anyOf must contain at least one schema".into()));
		};
		if variants.is_empty() {
			return Err(StrictSchemaError("anyOf must contain at least one schema".into()));
		}
		for variant in variants {
			if is_structured_schema(variant) {
				return Err(StrictSchemaError("object and array unions are unsupported".into()));
			}
			make_json_schema_node_strict(variant, is_unsupported_keyword)?;
		}
	}

	if let Some(items) = schema.get_mut("items") {
		if items.is_array() {
			return Err(StrictSchemaError("tuple schemas are unsupported".into()));
		}
		make_json_schema_node_strict(items, is_unsupported_keyword)?;
	}

	let is_object_schema = schema.get("type").and_then(Value::as_str) == Some("object");
	if schema.contains_key("properties") && !is_object_schema {
		return Err(StrictSchemaError("properties require type object".into()));
	}
	if !is_object_schema {
		return Ok(());
	}
	if schema
		.get("additionalProperties")
		.is_some_and(|value| *value != Value::Bool(false))
	{
		return Err(StrictSchemaError(
			"schema-valued or true additionalProperties is unsupported".into(),
		));
	}
	if schema.get("properties").is_some_and(|value| !value.is_object()) {
		return Err(StrictSchemaError("object properties must be a schema map".into()));
	}
	if let Some(required) = schema.get("required")
		&& !matches!(required, Value::Array(keys) if keys.iter().all(Value::is_string))
	{
		return Err(StrictSchemaError("object required must be a string array".into()));
	}

	let required: Vec<String> = match schema.get("required") {
		Some(Value::Array(keys)) => keys.iter().filter_map(Value::as_str).map(str::to_string).collect(),
		_ => Vec::new(),
	};
	let property_names: Vec<String> = match schema.get("properties") {
		Some(Value::Object(properties)) => properties.keys().cloned().collect(),
		_ => Vec::new(),
	};
	if required.iter().any(|key| !property_names.contains(key)) {
		return Err(StrictSchemaError("required contains an unknown property".into()));
	}
	if let Some(Value::Object(properties)) = schema.get_mut("properties") {
		for key in &property_names {
			let property = properties.get_mut(key).expect("property names were read from this map");
			make_json_schema_node_strict(property, is_unsupported_keyword)?;
			if !required.contains(key) && !schema_allows_null(property) {
				let property_schema = property.take();
				*property = json!({ "anyOf": [property_schema, { "type": "null" }] });
			}
		}
	}
	schema.insert(
		"required".into(),
		Value::Array(property_names.into_iter().map(Value::String).collect()),
	);
	schema.insert("additionalProperties".into(), Value::Bool(false));
	Ok(())
}

/// Convert a tool schema to the strict subset expected by provider constrained sampling.
fn make_strict_json_schema_inner(
	schema: &Value,
	is_unsupported_keyword: Option<&UnsupportedStrictSchemaKeywordCheck>,
) -> Result<Map<String, Value>, StrictSchemaError> {
	let mut cloned = schema.clone();
	if !cloned.is_object() {
		return Err(StrictSchemaError("root schema must have type object".into()));
	}
	pi_host::js::js_key_order(&mut cloned);
	make_json_schema_node_strict(&mut cloned, is_unsupported_keyword)?;
	match cloned {
		Value::Object(cloned) if cloned.get("type").and_then(Value::as_str) == Some("object") => Ok(cloned),
		_ => Err(StrictSchemaError("root schema must have type object".into())),
	}
}

/// Convert a tool schema to the strict subset expected by provider constrained sampling.
pub fn make_strict_json_schema(
	schema: &Value,
	is_unsupported_keyword: Option<&UnsupportedStrictSchemaKeywordCheck>,
) -> PiResult<Map<String, Value>> {
	make_strict_json_schema_inner(schema, is_unsupported_keyword).map_err(|error| PiError::new(error.0))
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
		return make_strict_json_schema(&tool.parameters, None).map(Value::Object);
	}
	Ok(tool.parameters.clone())
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
			Err(_) if *strict != ConstrainedSamplingStrictness::Require => Ok(None),
			Err(error) => Err(PiError::new(format!(
				"Tool \"{}\" requires JSON-schema constrained sampling, but {}.",
				tool.name, error.0
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
