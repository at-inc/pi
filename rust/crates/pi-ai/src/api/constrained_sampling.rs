//! Port of `packages/ai/src/api/constrained-sampling.ts`.
//! PORT-STATUS: pending

// PORT-STUB: w3-google-mistral
// The Google and Mistral adapters call `resolve_json_schema_strict_sampling` and `get_json_schema_tool_parameters`.
// These stubs follow the TypeScript bodies so the adapters and their tests run before w3-bedrock-misc ports this
// file; that shard replaces the whole file. `UnsupportedStrictJsonSchemaError` is a `PiError` with that `name`.
// `structuredClone` of a JS object keeps JS key order, so `make_strict_json_schema` applies `js_key_order` to its
// copy before reading `Object.keys`.

use pi_host::js::{js_key_order, json_stringify};
use pi_host::{PiError, PiResult};
use serde_json::{Map, Value, json};

use crate::types::{ConstrainedSamplingConfig, ConstrainedSamplingStrictness, Tool, ToolConstrainedSampling};

/// Returns true when a provider's strict mode rejects this schema keyword with this value.
pub type UnsupportedStrictSchemaKeywordCheck = dyn Fn(&str, &Value) -> bool;

const UNSUPPORTED_STRICT_JSON_SCHEMA_ERROR: &str = "UnsupportedStrictJsonSchemaError";

fn unsupported(message: impl Into<String>) -> PiError {
	PiError::named(UNSUPPORTED_STRICT_JSON_SCHEMA_ERROR, message)
}

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
	let Some(schema) = schema.as_object() else {
		return false;
	};
	let types: Vec<&Value> = match schema.get("type") {
		Some(kind @ Value::String(_)) => vec![kind],
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
		Some(Value::String(kind)) if kind == "null" => return true,
		Some(Value::Array(types)) if types.iter().any(|kind| kind.as_str() == Some("null")) => return true,
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

fn make_json_schema_node_strict(
	schema: &mut Value,
	is_unsupported_keyword: Option<&UnsupportedStrictSchemaKeywordCheck>,
) -> PiResult<()> {
	let Some(object) = schema.as_object_mut() else {
		return Err(unsupported("boolean schemas are unsupported"));
	};
	for key in UNSUPPORTED_STRICT_SCHEMA_KEYS {
		if object.contains_key(key) {
			return Err(unsupported(format!("{key} schemas are unsupported")));
		}
	}
	if let Some(is_unsupported_keyword) = is_unsupported_keyword {
		for (key, value) in object.iter() {
			if is_unsupported_keyword(key, value) {
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
	if object
		.get("properties")
		.is_some_and(|properties| !properties.is_object())
	{
		return Err(unsupported("object properties must be a schema map"));
	}
	if let Some(required) = object.get("required")
		&& !required
			.as_array()
			.is_some_and(|keys| keys.iter().all(Value::is_string))
	{
		return Err(unsupported("object required must be a string array"));
	}

	let required: Vec<String> = object
		.get("required")
		.and_then(Value::as_array)
		.map(|keys| keys.iter().filter_map(Value::as_str).map(str::to_string).collect())
		.unwrap_or_default();
	let mut detached = Map::new();
	let properties = match object.get_mut("properties") {
		Some(Value::Object(properties)) => properties,
		_ => &mut detached,
	};
	let property_names: Vec<String> = properties.keys().cloned().collect();
	if required.iter().any(|key| !property_names.contains(key)) {
		return Err(unsupported("required contains an unknown property"));
	}
	for (key, property) in properties.iter_mut() {
		make_json_schema_node_strict(property, is_unsupported_keyword)?;
		if !required.contains(key) && !schema_allows_null(property) {
			*property = json!({ "anyOf": [property.take(), { "type": "null" }] });
		}
	}
	object.insert(
		"required".into(),
		Value::Array(property_names.into_iter().map(Value::String).collect()),
	);
	object.insert("additionalProperties".into(), Value::Bool(false));
	Ok(())
}

/// Convert a tool schema to the strict subset expected by provider constrained sampling.
pub fn make_strict_json_schema(
	schema: &Value,
	is_unsupported_keyword: Option<&UnsupportedStrictSchemaKeywordCheck>,
) -> PiResult<Map<String, Value>> {
	let mut cloned = schema.clone();
	js_key_order(&mut cloned);
	if !cloned.is_object() {
		return Err(unsupported("root schema must have type object"));
	}
	make_json_schema_node_strict(&mut cloned, is_unsupported_keyword)?;
	match cloned {
		Value::Object(object) if object.get("type").and_then(Value::as_str) == Some("object") => Ok(object),
		_ => Err(unsupported("root schema must have type object")),
	}
}

/// `getJsonSchemaToolParameters(tool, strict)`.
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
		return match make_strict_json_schema(&tool.parameters, is_unsupported_keyword) {
			Ok(_) => Ok(Some(true)),
			Err(error) => {
				if *strict != ConstrainedSamplingStrictness::Require {
					return Ok(None);
				}
				Err(PiError::new(format!(
					"Tool \"{}\" requires JSON-schema constrained sampling, but {}.",
					tool.name, error.message
				)))
			}
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
