//! Port of `packages/schema/src/types.ts`.
//! PORT-STATUS: ported
//!
//! Public types: the schema accepted by compile(), its options, and the errors a failed value produces.
//!
//! Rust-only addition: [`Node`], [`SchemaObject`] and [`js_order`] (JSON objects iterate in JavaScript's key order).
//! [`Node`] and [`SchemaObject`]: the TypeScript compiler keys its caches, resource index and memo
//! tables by schema object identity (`Map<object, ...>`). A `serde_json::Value` tree has no stable identity that a
//! closure built from it may keep, so compile() first copies the schema into a [`Node`] tree whose objects sit behind
//! an `Rc`; the identity of a schema object is the address of its `Rc`. `ParseResult<T>` and `Static<T>` lose their
//! type parameter (it has no runtime effect).

use std::fmt;
use std::rc::Rc;

use indexmap::IndexMap;
use serde::Serialize;
use serde_json::Value;

/// A JSON Schema: `true`, `false` or a schema object. Schemas come from tool definitions, MCP servers and other
/// untrusted sources, so compile() reads every keyword defensively instead of trusting a static shape.
pub type JsonSchema = Value;

/// A format check: whether `value` (a string) has the format.
pub type FormatCheck = Rc<dyn Fn(&str) -> bool>;

/// The draft whose semantics apply.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Draft {
	/// `"2020-12"`.
	V2020_12,
	/// `"7"`.
	V7,
}

/// What a `$ref` that names nothing does.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum UnresolvedRefs {
	/// `"throw"`.
	Throw,
	/// `"reject"`.
	Reject,
}

/// Options of compile().
#[derive(Clone, Default)]
pub struct CompileOptions {
	/// The draft whose semantics apply: "2020-12" (default) or "7".
	pub draft: Option<Draft>,
	/// Format name to check. Without it `format` is an annotation, as draft 2020-12 specifies. The standard formats are
	/// in `pi_schema::formats`.
	pub formats: Option<Rc<IndexMap<String, FormatCheck>>>,
	/// Schema documents by URI, for `$ref`s to other documents.
	pub schemas: Option<Rc<IndexMap<String, JsonSchema>>>,
	/// Add `coerce()` and `parse()` to the validator, for loosely typed input (see the README's Coercion section).
	pub coerce: Option<bool>,
	/// What a `$ref` that names nothing (a missing definition, a document not in `schemas`) does: `"throw"` (default)
	/// makes compile() throw; `"reject"` compiles it as `false`, so only values at that place fail, with the error
	/// "unresolved reference".
	pub unresolved_refs: Option<UnresolvedRefs>,
}

impl fmt::Debug for CompileOptions {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		f.debug_struct("CompileOptions")
			.field("draft", &self.draft)
			.field(
				"formats",
				&self.formats.as_ref().map(|formats| formats.keys().collect::<Vec<_>>()),
			)
			.field(
				"schemas",
				&self.schemas.as_ref().map(|schemas| schemas.keys().collect::<Vec<_>>()),
			)
			.field("coerce", &self.coerce)
			.field("unresolved_refs", &self.unresolved_refs)
			.finish()
	}
}

/// The result of `parse()`: the coerced value when it is valid, else the errors of the coerced value.
#[derive(Clone, Debug, PartialEq)]
pub enum ParseResult {
	/// `{ ok: true, value }`.
	Ok(Value),
	/// `{ ok: false, errors }`.
	Err(Vec<ValidationError>),
}

impl ParseResult {
	/// `result.ok`.
	pub fn ok(&self) -> bool {
		matches!(self, ParseResult::Ok(_))
	}
}

/// One failure of a value: where it is (a JSON Pointer, "" for the value itself), the failing keyword and a message.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ValidationError {
	pub path: String,
	pub keyword: String,
	pub message: String,
}

/// A schema value as compile() reads it: the JSON of the schema, with every object behind an `Rc` so it has an
/// identity (see the file header).
#[derive(Clone, Debug)]
pub enum Node {
	Null,
	Bool(bool),
	Number(serde_json::Number),
	String(Rc<str>),
	Array(Rc<[Node]>),
	Object(SchemaObject),
}

/// A schema object as compile() reads it: any keyword, of any value, checked where it is used.
#[derive(Clone, Debug)]
pub struct SchemaObject(pub Rc<IndexMap<String, Node>>);

/// A compiled boolean check.
pub type Check = Rc<dyn Fn(&Value) -> bool>;

impl Node {
	/// The schema tree of a JSON value.
	pub fn from_value(value: &Value) -> Node {
		match value {
			Value::Null => Node::Null,
			Value::Bool(b) => Node::Bool(*b),
			Value::Number(n) => Node::Number(js_number(n)),
			Value::String(s) => Node::String(Rc::from(s.as_str())),
			Value::Array(items) => Node::Array(items.iter().map(Node::from_value).collect()),
			Value::Object(map) => Node::Object(SchemaObject(Rc::new(
				js_order(map.iter())
					.into_iter()
					.map(|(k, v)| (k.clone(), Node::from_value(v)))
					.collect(),
			))),
		}
	}

	/// The JSON value of this schema tree.
	pub fn to_value(&self) -> Value {
		match self {
			Node::Null => Value::Null,
			Node::Bool(b) => Value::Bool(*b),
			Node::Number(n) => Value::Number(n.clone()),
			Node::String(s) => Value::String(s.to_string()),
			Node::Array(items) => Value::Array(items.iter().map(Node::to_value).collect()),
			Node::Object(object) => Value::Object(object.0.iter().map(|(k, v)| (k.clone(), v.to_value())).collect()),
		}
	}

	/// `node === true`.
	pub fn is_true(&self) -> bool {
		matches!(self, Node::Bool(true))
	}

	/// `node === false`.
	pub fn is_false(&self) -> bool {
		matches!(self, Node::Bool(false))
	}

	/// The schema object, when this is one (`isSchemaObject`).
	pub fn as_object(&self) -> Option<&SchemaObject> {
		match self {
			Node::Object(object) => Some(object),
			_ => None,
		}
	}

	/// `typeof node === "string"`.
	pub fn as_str(&self) -> Option<&str> {
		match self {
			Node::String(s) => Some(s),
			_ => None,
		}
	}

	/// `typeof node === "number"`.
	pub fn as_f64(&self) -> Option<f64> {
		match self {
			Node::Number(n) => n.as_f64(),
			_ => None,
		}
	}

	/// `Array.isArray(node)`.
	pub fn as_array(&self) -> Option<&[Node]> {
		match self {
			Node::Array(items) => Some(items),
			_ => None,
		}
	}

	/// `typeof node === "object" && node !== null` (an object or an array).
	pub fn is_object_like(&self) -> bool {
		matches!(self, Node::Object(_) | Node::Array(_))
	}
}

impl SchemaObject {
	/// `node[key]`, `None` when absent (undefined).
	pub fn get(&self, key: &str) -> Option<&Node> {
		self.0.get(key)
	}

	/// `Object.hasOwn(node, key)`.
	pub fn has(&self, key: &str) -> bool {
		self.0.contains_key(key)
	}

	/// `node[key]` when it is a string.
	pub fn str(&self, key: &str) -> Option<&str> {
		self.get(key).and_then(Node::as_str)
	}

	/// `node[key]` when it is a number.
	pub fn number(&self, key: &str) -> Option<f64> {
		self.get(key).and_then(Node::as_f64)
	}

	/// `node[key]` when it is an array.
	pub fn array(&self, key: &str) -> Option<&[Node]> {
		self.get(key).and_then(Node::as_array)
	}

	/// `node[key]` when it is a schema object.
	pub fn object(&self, key: &str) -> Option<&SchemaObject> {
		self.get(key).and_then(Node::as_object)
	}

	/// `Object.keys(node)`.
	pub fn keys(&self) -> impl Iterator<Item = &String> {
		self.0.keys()
	}

	/// `Object.entries(node)`.
	pub fn entries(&self) -> impl Iterator<Item = (&String, &Node)> {
		self.0.iter()
	}

	/// The number of keys.
	pub fn len(&self) -> usize {
		self.0.len()
	}

	/// Whether the object has no keys.
	pub fn is_empty(&self) -> bool {
		self.0.is_empty()
	}

	/// The identity of this schema object (the address of its `Rc`), the key of identity-keyed tables. Tables keyed
	/// by it also keep the object alive, so an address is never reused for another object while it is a key.
	pub fn id(&self) -> usize {
		Rc::as_ptr(&self.0) as *const () as usize
	}

	/// `a === b` for schema objects.
	pub fn same(&self, other: &SchemaObject) -> bool {
		Rc::ptr_eq(&self.0, &other.0)
	}

	/// A new schema object of `entries`.
	pub fn from_entries(entries: IndexMap<String, Node>) -> SchemaObject {
		SchemaObject(Rc::new(entries))
	}
}

/// `entries` of an object in JavaScript's property order: array-index keys (`"0"`, `"1"`, ...) first in ascending
/// order, then the other keys in insertion order. Schemas and values are JSON objects, whose `Object.keys` order this is.
pub fn js_order<'a, T>(entries: impl Iterator<Item = (&'a String, T)>) -> Vec<(&'a String, T)> {
	let mut entries: Vec<_> = entries.collect();
	if entries.iter().any(|(key, _)| array_index(key).is_some()) {
		entries.sort_by_key(|(key, _)| array_index(key).map_or((1, 0), |index| (0, index)));
	}
	entries
}

// A canonical array index: "0", or digits without a leading zero below 2^32 - 1.
fn array_index(key: &str) -> Option<u32> {
	if key != "0" && (key.starts_with('0') || key.is_empty() || !key.bytes().all(|c| c.is_ascii_digit())) {
		return None;
	}
	key.parse::<u32>().ok().filter(|index| *index != u32::MAX)
}

// A JSON number as JavaScript reads it: an integer beyond 2^53 becomes the nearest double.
fn js_number(n: &serde_json::Number) -> serde_json::Number {
	match (n.as_i64(), n.as_u64(), n.as_f64()) {
		(Some(i), _, _) if i.unsigned_abs() <= 1 << 53 => n.clone(),
		(_, Some(u), _) if u <= 1 << 53 => n.clone(),
		(_, _, Some(x)) if n.is_i64() || n.is_u64() => serde_json::Number::from_f64(x).unwrap_or_else(|| n.clone()),
		_ => n.clone(),
	}
}
