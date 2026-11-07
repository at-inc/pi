//! Port of `packages/schema/src/builders.ts`.
//! PORT-STATUS: ported
//!
//! Schema builders: short functions that return plain JSON Schema objects. The objects are ordinary data: they
//! serialize with JSON.stringify and compile with compile() like any other schema.
//!
//! Rust port: the static types the TypeScript builders carry (`Schema<T>`'s brand, `Static<S>`, `StaticObject<P>`) have
//! no runtime effect and are dropped; `Schema` is a JSON value. The option bags (`Annotations`, `LengthBounds`,
//! `NumberBounds` and each builder's extra keys) are JSON objects spread into the schema as
//! `{ type: "string", ...options }` spreads them, so the schema's keys come in JavaScript order (array-index keys
//! first, ascending); a value that is not an object spreads nothing (as `undefined` and `null` do). `Properties` is a
//! list of named `Property` values, iterated in the same order `Object.entries` gives.

use indexmap::IndexMap;
use serde_json::{Map, Value};

use crate::types::js_order;

/// A JSON Schema object (the TypeScript type also carries the type of its valid values).
pub type Schema = Value;

/// A string, with optional length bounds (in code points), pattern and format.
pub fn str(options: Value) -> Schema {
	with_type("string", options)
}

/// A number within optional bounds.
pub fn num(options: Value) -> Schema {
	with_type("number", options)
}

/// An integer within optional bounds.
pub fn int(options: Value) -> Schema {
	with_type("integer", options)
}

/// A boolean.
pub fn bool(options: Value) -> Schema {
	with_type("boolean", options)
}

/// `null`.
pub fn nul(options: Value) -> Schema {
	with_type("null", options)
}

/// Any value.
pub fn unknown(options: Value) -> Schema {
	object(Map::new(), options)
}

/// Exactly `value` (a string, number, boolean or null).
pub fn lit(value: Value, options: Value) -> Schema {
	object(Map::from_iter([("const".to_string(), value)]), options)
}

/// One of the given values.
pub fn one_of(values: Vec<Value>, options: Value) -> Schema {
	object(Map::from_iter([("enum".to_string(), Value::Array(values))]), options)
}

/// Any of the given schemas. A union of objects tagged by a `lit()` property (`{ kind: lit("text"), ... }`) is checked
/// by the tag: only the branch it selects runs, and its errors are the ones reported.
pub fn union(schemas: Vec<Schema>, options: Value) -> Schema {
	object(Map::from_iter([("anyOf".to_string(), Value::Array(schemas))]), options)
}

/// `schema` or `null`.
pub fn nullable(schema: Schema, options: Value) -> Schema {
	let null = with_type("null", Value::Null);
	object(
		Map::from_iter([("anyOf".to_string(), Value::Array(vec![schema, null]))]),
		options,
	)
}

/// An array of `items`, with optional length bounds and unique items.
pub fn arr(items: Schema, options: Value) -> Schema {
	object(
		Map::from_iter([("type".to_string(), Value::from("array")), ("items".to_string(), items)]),
		options,
	)
}

/// A fixed-length array whose items match `items` in order.
pub fn tuple(items: Vec<Schema>, options: Value) -> Schema {
	let length = items.len();
	object(
		Map::from_iter([
			("type".to_string(), Value::from("array")),
			("prefixItems".to_string(), Value::Array(items)),
			("items".to_string(), Value::Bool(false)),
			("minItems".to_string(), Value::from(length)),
		]),
		options,
	)
}

/// An object with string keys and values of `value` (a record).
pub fn map(value: Schema, options: Value) -> Schema {
	object(
		Map::from_iter([
			("type".to_string(), Value::from("object")),
			("additionalProperties".to_string(), value),
		]),
		options,
	)
}

/// A Uint8Array with optional length bounds (an extension of JSON Schema for values that never pass through JSON).
pub fn bytes(options: Value) -> Schema {
	with_type("bytes", options)
}

/// An optional property of obj(): `{ name: opt(str()) }` may omit `name` or set it to undefined.
#[derive(Clone, Debug, PartialEq)]
pub struct Optional {
	pub schema: Schema,
}

impl Optional {
	/// `new Optional(schema)`.
	pub fn new(schema: Schema) -> Optional {
		Optional { schema }
	}
}

/// Marks an obj() property as optional.
pub fn opt(schema: Schema) -> Optional {
	Optional::new(schema)
}

/// A property of obj(): a schema or an opt() schema.
#[derive(Clone, Debug, PartialEq)]
pub enum Property {
	Schema(Schema),
	Optional(Optional),
}

impl From<Schema> for Property {
	fn from(schema: Schema) -> Property {
		Property::Schema(schema)
	}
}

impl From<Optional> for Property {
	fn from(optional: Optional) -> Property {
		Property::Optional(optional)
	}
}

/// The properties of obj(): a schema or an opt() schema per name, in order.
pub type Properties = IndexMap<String, Property>;

/// A closed object: the given properties (required unless opt()) and no others. `open: true` allows other
/// properties, as tool schemas from other systems often do.
pub fn obj<K: Into<String>>(properties: impl IntoIterator<Item = (K, Property)>, options: Value) -> Schema {
	let mut annotations = match options {
		Value::Object(options) => options,
		_ => Map::new(),
	};
	let open = annotations.shift_remove("open");
	let properties: Properties = properties
		.into_iter()
		.map(|(name, property)| (name.into(), property))
		.collect();
	let mut props = Map::new();
	let mut required = Vec::new();
	for (name, property) in js_order(properties.iter()) {
		match property {
			Property::Optional(optional) => {
				props.insert(name.clone(), optional.schema.clone());
			}
			Property::Schema(schema) => {
				props.insert(name.clone(), schema.clone());
				required.push(Value::String(name.clone()));
			}
		}
	}
	let mut out = Map::new();
	out.insert("type".into(), Value::from("object"));
	out.insert("properties".into(), Value::Object(props));
	if !required.is_empty() {
		out.insert("required".into(), Value::Array(required));
	}
	if open != Some(Value::Bool(true)) {
		out.insert("additionalProperties".into(), Value::Bool(false));
	}
	object(out, Value::Object(annotations))
}

fn with_type(name: &str, options: Value) -> Schema {
	object(Map::from_iter([("type".to_string(), Value::from(name))]), options)
}

/// `{ ...entries, ...options }`, in JavaScript key order.
fn object(mut entries: Map<String, Value>, options: Value) -> Schema {
	if let Value::Object(options) = options {
		for (key, value) in options {
			entries.insert(key, value);
		}
	}
	Value::Object(
		js_order(entries.iter())
			.into_iter()
			.map(|(key, value)| (key.clone(), value.clone()))
			.collect(),
	)
}
