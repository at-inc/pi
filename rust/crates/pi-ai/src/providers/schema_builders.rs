//! Port of `packages/ai/src/providers/schema-builders.ts`.
//! PORT-STATUS: ported
//!
//! pi-schema's builders return plain JSON Schema objects, and these schemas are only used as data, so the Rust port
//! builds them as `serde_json::Value`s. `Property`, `Properties` and `opt` stand in for pi-schema's `Optional`,
//! `Properties` and `opt()`: the object builder below needs to tell required properties from optional ones. The
//! `Static<T>` types the TS builders carry are dropped; the Rust value types live next to each schema.

// Builders for pi-ai's own configuration schemas (model metadata, compat settings): pi-schema's builders, shaped like the
// TypeBox schemas they replace, so models.json and the catalog validate exactly as before.

use indexmap::IndexMap;
use serde_json::{Map, Value};

/// A property of an object schema: required, or optional (pi-schema's `opt()`).
#[derive(Clone, Debug, PartialEq)]
pub enum Property {
	Required(Value),
	Optional(Value),
}

impl Property {
	/// The property's schema.
	pub fn schema(&self) -> &Value {
		match self {
			Property::Required(schema) | Property::Optional(schema) => schema,
		}
	}
}

impl From<Value> for Property {
	fn from(schema: Value) -> Self {
		Property::Required(schema)
	}
}

/// The properties of an object schema, in declaration order.
pub type Properties = IndexMap<String, Property>;

/// Marks an object property as optional (pi-schema's `opt()`).
pub fn opt(schema: Value) -> Property {
	Property::Optional(schema)
}

/// An object that allows other properties; `additionalProperties: true` says so explicitly.
pub fn open_object(properties: &Properties, options: Value) -> Value {
	let mut annotations = into_object(options);
	let additional_properties = annotations.shift_remove("additionalProperties");
	let mut props = Map::new();
	let mut required = Vec::new();
	for (name, property) in properties {
		match property {
			Property::Optional(schema) => {
				props.insert(name.clone(), schema.clone());
			}
			Property::Required(schema) => {
				props.insert(name.clone(), schema.clone());
				required.push(Value::String(name.clone()));
			}
		}
	}
	let mut schema = Map::new();
	schema.insert("type".into(), "object".into());
	schema.insert("properties".into(), Value::Object(props));
	if !required.is_empty() {
		schema.insert("required".into(), Value::Array(required));
	}
	schema.extend(annotations);
	if additional_properties == Some(Value::Bool(true)) {
		schema.insert("additionalProperties".into(), Value::Bool(true));
	}
	Value::Object(schema)
}

/// One string: `{ type: "string", const: value }`.
pub fn string_literal(value: &str, options: Value) -> Value {
	let mut schema = Map::new();
	schema.insert("type".into(), "string".into());
	schema.insert("const".into(), value.into());
	schema.extend(into_object(options));
	Value::Object(schema)
}

/// An object with any string keys and values of `value`: `{ type: "object", patternProperties: { "^.*$": value } }`.
pub fn string_record(value: Value, options: Value) -> Value {
	let mut pattern_properties = Map::new();
	pattern_properties.insert("^.*$".into(), value);
	let mut schema = Map::new();
	schema.insert("type".into(), "object".into());
	schema.insert("patternProperties".into(), Value::Object(pattern_properties));
	schema.extend(into_object(options));
	Value::Object(schema)
}

fn into_object(options: Value) -> Map<String, Value> {
	match options {
		Value::Object(map) => map,
		_ => Map::new(),
	}
}
