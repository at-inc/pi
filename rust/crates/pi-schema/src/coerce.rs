//! Port of `packages/schema/src/coerce.ts`.
//! PORT-STATUS: ported
//!
//! Coercion: turns a loosely typed value (a model's tool call arguments, form input, query strings) into the value a
//! schema most likely meant, before it is checked. Only validators compiled with `{ coerce: true }` have it, and they
//! build it on their first coerce() or parse(); the check path never runs it.
//!
//! Three passes, in this order:
//! 1. Optional nulls: a `null` in an optional property whose schema rejects null is dropped, as if omitted.
//! 2. Conversion rules ported from TypeBox 1.3.27's Value.Convert (MIT), keyed on JSON Schema structure (`$ref`, `const`,
//!    `enum`, `anyOf`/`oneOf`, `allOf`, `type`, `items`/`prefixItems`, `properties`/`patternProperties`) instead of
//!    TypeBox's kind marks: `null` becomes "null" for a string, `"3.5"` becomes 3 for an integer, a single value becomes
//!    a one-item array, and a union takes the first branch whose conversion passes.
//! 3. Primitive coercion by `type` (also type arrays), as in Ajv's `coerceTypes`: what pass 2 left, for example a type
//!    array `["integer", "null"]`, or `""` for `null`.
//!
//! A value that cannot be coerced is left as it is; the check then reports it.
//!
//! Every pass is copy-on-write: an object or array is copied only when one of its members changes, so an unchanged
//! value comes back as the same reference and the input is never modified. All passes walk the same keywords for
//! members: `properties`, then `patternProperties`, then `additionalProperties` for keys neither names; tuple items by
//! `prefixItems` (or a draft-7 `items` array), the rest by `items`. Draft 7's `additionalItems` is not coerced.
//!
//! Rust port: "the same reference" is `None` (every pass returns `Some(value)` only for a changed value) and coerce()
//! returns `Cow::Borrowed` for an unchanged value. JSON has no `undefined` and no `bigint`, so their branches are gone.
//! The re-entry stack compares objects and arrays by address, as the TypeScript compares them by reference; the memo
//! keys an object or array by its JSON text instead of its reference (a converted value may be dropped and its address
//! reused within one call; conversion without cuts depends only on the node and the value's content, so equal content
//! gives the same result). Properties are own properties: `key in value` never sees `Object.prototype` members.

use std::borrow::Cow;
use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use pi_host::PiResult;
use pi_host::js::{json_stringify, number_to_string};
use serde_json::{Map, Value};

use crate::checks::{PrimitiveKey, is_integer, strict_equals};
use crate::compiler::Compiler;
use crate::shared::{JsRegex, shared_regex};
use crate::types::{Node, SchemaObject};

/// Coerces values for one compiled schema.
pub struct Coercer {
	c: Compiler,
	schema: Node,
	/// The (schema node, value) pairs pass 2 is converting through a re-entry point, as a stack. Conversion can enter
	/// a node again with the same value only through a `$ref` or by wrapping a single value into an array (also for
	/// schemas that cycle through shared JS objects): a recursive schema that meets a value no branch accepts would
	/// convert it without end. A pair already on the stack (primitives by value, objects by reference) leaves the value
	/// unchanged. Only those two points push, so schemas without references or wraps pay nothing.
	entered: RefCell<Vec<(usize, Entered)>>,
	/// How often a re-entry was cut short in this coerce() call; results computed across one are not memoized.
	cuts: Cell<usize>,
	/// Results of `$ref` targets by (node, value) within one coerce() call: a recursive union whose branches all
	/// convert the same children (and all fail) would convert them once per branch, exponentially in the depth.
	memo: RefCell<Option<Memo>>,
}

type Memo = HashMap<usize, HashMap<MemoKey, Option<Value>>>;

/// A value on the re-entry stack: a primitive by value, an object or array by its address.
enum Entered {
	Primitive(Value),
	Object(usize),
}

/// A value as a memo key: a primitive by value (SameValueZero), an object or array by its JSON text.
#[derive(PartialEq, Eq, Hash)]
enum MemoKey {
	Primitive(PrimitiveKey),
	Json(String),
}

impl Coercer {
	/// `new Coercer(compiler, schema)`.
	pub fn new(compiler: Compiler, schema: Node) -> Coercer {
		Coercer {
			c: compiler,
			schema,
			entered: RefCell::new(Vec::new()),
			cuts: Cell::new(0),
			memo: RefCell::new(None),
		}
	}

	/// `value` coerced: a copy along the changed paths only, or `value` itself when nothing changes.
	pub fn coerce<'a>(&self, value: &'a Value) -> PiResult<Cow<'a, Value>> {
		let base = self.c.r().root_base.clone();
		let without_nulls = self.drop_optional_nulls(value, &self.schema, &base)?;
		let without_nulls_value = without_nulls.as_ref().unwrap_or(value);
		let converted = self.convert_root(without_nulls_value)?;
		let converted_value = converted.as_ref().unwrap_or(without_nulls_value);
		let typed = self.coerce_types(converted_value, &self.schema, &base)?;
		Ok(match typed.or(converted).or(without_nulls) {
			Some(next) => Cow::Owned(next),
			None => Cow::Borrowed(value),
		})
	}

	/// Pass 2 alone, as TypeBox's `Value.Convert` (the TypeBox conformance tests run it).
	pub fn convert_only<'a>(&self, value: &'a Value) -> PiResult<Cow<'a, Value>> {
		Ok(match self.convert_root(value)? {
			Some(next) => Cow::Owned(next),
			None => Cow::Borrowed(value),
		})
	}

	fn convert_root(&self, value: &Value) -> PiResult<Option<Value>> {
		let result = self.convert(&self.schema, &self.c.r().root_base, value);
		*self.memo.borrow_mut() = None;
		self.cuts.set(0);
		result
	}

	/// `node` converted for `value` at a re-entry point: unchanged when that pair is already being converted.
	fn reenter(&self, node: &SchemaObject, base: &str, value: &Value, memoize: bool) -> PiResult<Option<Value>> {
		let id = node.id();
		if self.entered.borrow().iter().any(|(n, v)| *n == id && v.is(value)) {
			self.cuts.set(self.cuts.get() + 1);
			return Ok(None);
		}
		let key = if memoize { Some(memo_key(value)) } else { None };
		if let Some(key) = &key {
			let known = self
				.memo
				.borrow()
				.as_ref()
				.and_then(|memo| memo.get(&id))
				.and_then(|values| values.get(key))
				.cloned();
			if let Some(known) = known {
				return Ok(known);
			}
		}
		let cuts = self.cuts.get();
		self.entered.borrow_mut().push((id, Entered::of(value)));
		let result = self.convert(&Node::Object(node.clone()), base, value);
		self.entered.borrow_mut().pop();
		let result = result?;
		if let Some(key) = key
			&& self.cuts.get() == cuts
		{
			self.memo
				.borrow_mut()
				.get_or_insert_with(HashMap::new)
				.entry(id)
				.or_default()
				.insert(key, result.clone());
		}
		Ok(result)
	}

	/// Whether `value` passes the schema `node`; `None` when `node` cannot be compiled.
	fn passes(&self, node: &Node, base: &str, value: &Value) -> Option<bool> {
		self.c
			.check(node, &self.base_of(node, base))
			.ok()
			.map(|check| check(value))
	}

	fn base_of(&self, node: &Node, base: &str) -> String {
		node.as_object()
			.and_then(|node| self.c.r().base_of(node))
			.unwrap_or_else(|| base.to_string())
	}

	// ---------------------------------------------------------------------------------------------------------------
	// Pass 1: optional nulls
	// ---------------------------------------------------------------------------------------------------------------

	fn drop_optional_nulls(&self, value: &Value, schema: &Node, base: &str) -> PiResult<Option<Value>> {
		let Some(object) = schema.as_object() else {
			return Ok(None);
		};
		let b = self.base_of(schema, base);
		if let Some(items) = value.as_array() {
			return map_items(items, |item, i| match item_schema_at(object, i) {
				None => Ok(None),
				Some(item_schema) => self.drop_optional_nulls(item, item_schema, &b),
			});
		}
		let Some(value) = value.as_object() else {
			return Ok(None);
		};
		let properties = object.object("properties");
		let patterns = patterns_of(object)?;
		let additional = object
			.get("additionalProperties")
			.filter(|additional| additional.as_object().is_some());
		if patterns.is_empty() && additional.is_none() && properties.is_none() {
			return Ok(None);
		}
		let required: HashSet<&str> = object
			.array("required")
			.unwrap_or(&[])
			.iter()
			.filter_map(Node::as_str)
			.collect();
		let mut out: Option<Map<String, Value>> = None;
		for (key, property) in properties.into_iter().flat_map(SchemaObject::entries) {
			let Some(current) = value.get(key) else {
				continue;
			};
			let is_ref = property
				.as_object()
				.is_some_and(|property| property.str("$ref").is_some());
			if current.is_null()
				&& !required.contains(key.as_str())
				&& !is_ref && self.passes(property, &b, &Value::Null) == Some(false)
			{
				out.get_or_insert_with(|| value.clone()).shift_remove(key);
				continue;
			}
			let next = self.drop_optional_nulls(current, property, &b)?;
			set(&mut out, value, key, next);
		}
		// Values of pattern and additional properties are walked for their own optional nulls; the keys themselves
		// are not optional properties, so they stay.
		if !patterns.is_empty() || additional.is_some() {
			for key in value.keys() {
				let mut matched = properties.is_some_and(|properties| properties.has(key));
				for (pattern, item) in &patterns {
					if !pattern.test(key) {
						continue;
					}
					matched = true;
					let Some(current) = out.as_ref().unwrap_or(value).get(key).cloned() else {
						continue;
					};
					let next = self.drop_optional_nulls(&current, item, &b)?;
					set(&mut out, value, key, next);
				}
				if !matched && let Some(additional) = additional {
					let next = self.drop_optional_nulls(&value[key], additional, &b)?;
					set(&mut out, value, key, next);
				}
			}
		}
		Ok(out.map(Value::Object))
	}

	// ---------------------------------------------------------------------------------------------------------------
	// Pass 2: TypeBox's conversion rules
	// ---------------------------------------------------------------------------------------------------------------

	fn convert(&self, node: &Node, base: &str, value: &Value) -> PiResult<Option<Value>> {
		let Some(object) = node.as_object() else {
			return Ok(None);
		};
		let b = self.base_of(node, base);
		if let Some(reference) = object.str("$ref") {
			let Ok(target) = self.c.r().resolve(reference, &b) else {
				return Ok(None);
			};
			return match target.node.as_object() {
				Some(target_node) => self.reenter(target_node, &target.base, value, true),
				None => Ok(None),
			};
		}
		self.convert_node(object, &b, value)
	}

	/// One schema node's conversion: its first keyword among `const`, `enum`, the unions, `allOf` and `type`.
	fn convert_node(&self, node: &SchemaObject, b: &str, value: &Value) -> PiResult<Option<Value>> {
		if let Some(literal) = node.get("const") {
			return Ok(convert_literal(&literal.to_value(), value));
		}
		if let Some(values) = node.array("enum") {
			return self.convert_enum(node, values, b, value);
		}
		// A union or intersection node may also declare members (plain JSON Schema; TypeBox's never do): they convert
		// after the branches.
		if let Some(branches) = node.array("anyOf") {
			return self.convert_union(node, branches, b, value);
		}
		if let Some(branches) = node.array("oneOf") {
			return self.convert_union(node, branches, b, value);
		}
		if let Some(members) = node.array("allOf") {
			let mut next: Option<Value> = None;
			for member in members {
				let converted = self.convert(member, b, next.as_ref().unwrap_or(value))?;
				if converted.is_some() {
					next = converted;
				}
			}
			let members = self.convert_members(node, b, next.as_ref().unwrap_or(value))?;
			return Ok(settle(members.or(next), value));
		}
		match node.get("type") {
			Some(Node::String(t)) => match &**t {
				"string" => Ok(try_string(value).and_then(|next| settle(Some(next), value))),
				"number" => Ok(try_number(value).and_then(|next| settle(Some(number(next)), value))),
				"integer" => Ok(try_number(value).and_then(|next| settle(Some(number(next.trunc())), value))),
				"boolean" => Ok(try_boolean(value).and_then(|next| settle(Some(Value::Bool(next)), value))),
				"null" => Ok(try_null(value).and_then(|()| settle(Some(Value::Null), value))),
				"array" => self.convert_array(node, b, value),
				"object" => self.convert_object(node, b, value),
				_ => Ok(None),
			},
			None => {
				if node.has("properties") || node.has("patternProperties") {
					return self.convert_object(node, b, value);
				}
				if node.has("items") || node.has("prefixItems") {
					return self.convert_array(node, b, value);
				}
				Ok(None)
			}
			Some(_) => Ok(None),
		}
	}

	/// An enum is a union of its values: a member stays, else the first value the input converts to.
	fn convert_enum(&self, node: &SchemaObject, values: &[Node], base: &str, value: &Value) -> PiResult<Option<Value>> {
		let values: Vec<Value> = values.iter().map(Node::to_value).collect();
		if values.iter().any(|member| strict_equals(member, value)) {
			return Ok(None);
		}
		let whole = Node::Object(node.clone());
		for member in &values {
			if let Some(candidate) = convert_literal(member, value)
				&& self.passes(&whole, base, &candidate) == Some(true)
			{
				return Ok(Some(candidate));
			}
		}
		Ok(None)
	}

	/// A value one branch accepts stays; else the first branch's conversion that the whole union accepts. Branches are
	/// converted in order and only until one is accepted, so a later recursive branch is not entered needlessly. A node
	/// that also declares members (plain JSON Schema; TypeBox's never do) converts them into each candidate before it is
	/// checked, so a branch is not discarded for a sibling property that is converted after it.
	fn convert_union(
		&self,
		node: &SchemaObject,
		branches: &[Node],
		base: &str,
		value: &Value,
	) -> PiResult<Option<Value>> {
		if branches
			.iter()
			.any(|branch| self.passes(branch, base, value) == Some(true))
		{
			return self.convert_members(node, base, value);
		}
		let whole = Node::Object(node.clone());
		for branch in branches {
			let converted = self.convert(branch, base, value)?;
			let members = self.convert_members(node, base, converted.as_ref().unwrap_or(value))?;
			let candidate = members.or(converted);
			if self.passes(&whole, base, candidate.as_ref().unwrap_or(value)) == Some(true) {
				return Ok(settle(candidate, value));
			}
		}
		self.convert_members(node, base, value)
	}

	/// The object members of a union or intersection node, when it declares any.
	fn convert_members(&self, node: &SchemaObject, base: &str, value: &Value) -> PiResult<Option<Value>> {
		let declares = node.has("properties") || node.has("patternProperties") || node.has("additionalProperties");
		if declares {
			self.convert_object(node, base, value)
		} else {
			Ok(None)
		}
	}

	fn convert_array(&self, node: &SchemaObject, base: &str, value: &Value) -> PiResult<Option<Value>> {
		let prefix = node.array("items").or_else(|| node.array("prefixItems"));
		if let Some(prefix) = prefix {
			// A tuple converts its items and never wraps a single value.
			let Some(items) = value.as_array() else {
				return Ok(None);
			};
			let rest = if node.array("items").is_some() {
				None
			} else {
				node.get("items")
			};
			return map_items(items, |item, i| match prefix.get(i).or(rest) {
				None => Ok(None),
				Some(schema) => self.convert(schema, base, item),
			});
		}
		if let Some(items) = value.as_array() {
			return match node.get("items") {
				None => Ok(None),
				Some(schema) => map_items(items, |item, _| self.convert(schema, base, item)),
			};
		}
		// The wrapped single value is converted by the item schema with the same value: a re-entry point.
		let converted = match node.get("items").and_then(Node::as_object) {
			Some(item) => self.reenter(item, base, value, false)?,
			None => None,
		};
		Ok(Some(Value::Array(vec![converted.unwrap_or_else(|| value.clone())])))
	}

	fn convert_object(&self, node: &SchemaObject, base: &str, value: &Value) -> PiResult<Option<Value>> {
		let Some(value) = value.as_object() else {
			return Ok(None);
		};
		let properties = node.object("properties");
		let patterns = patterns_of(node)?;
		let additional = node
			.get("additionalProperties")
			.filter(|additional| additional.as_object().is_some());
		let mut out: Option<Map<String, Value>> = None;
		for (key, current) in value {
			let mut next: Option<Value> = None;
			let property = properties.and_then(|properties| properties.get(key));
			let mut matched = property.is_some();
			if let Some(property) = property {
				next = self.convert(property, base, current)?;
			}
			for (pattern, schema) in &patterns {
				if !pattern.test(key) {
					continue;
				}
				matched = true;
				let converted = self.convert(schema, base, next.as_ref().unwrap_or(current))?;
				if converted.is_some() {
					next = converted;
				}
			}
			if !matched && let Some(additional) = additional {
				next = self.convert(additional, base, current)?;
			}
			if let Some(next) = settle(next, current) {
				out.get_or_insert_with(|| value.clone()).insert(key.clone(), next);
			}
		}
		Ok(out.map(Value::Object))
	}

	// ---------------------------------------------------------------------------------------------------------------
	// Pass 3: primitive coercion by type
	// ---------------------------------------------------------------------------------------------------------------

	fn coerce_types(&self, value: &Value, schema: &Node, base: &str) -> PiResult<Option<Value>> {
		let Some(object) = schema.as_object() else {
			return Ok(None);
		};
		let b = self.base_of(schema, base);
		let mut next: Option<Value> = None;
		if let Some(members) = object.array("allOf") {
			for member in members {
				let coerced = self.coerce_types(next.as_ref().unwrap_or(value), member, &b)?;
				if coerced.is_some() {
					next = coerced;
				}
			}
		}
		if let Some(branches) = object.array("anyOf") {
			let coerced = self.coerce_union(next.as_ref().unwrap_or(value), branches, &b)?;
			if coerced.is_some() {
				next = coerced;
			}
		}
		if let Some(branches) = object.array("oneOf") {
			let coerced = self.coerce_union(next.as_ref().unwrap_or(value), branches, &b)?;
			if coerced.is_some() {
				next = coerced;
			}
		}
		let types = types_of(object);
		let current = next.as_ref().unwrap_or(value);
		let matches_union_member = types.len() > 1 && types.iter().any(|t| matches_type(current, t));
		if !types.is_empty() && !matches_union_member {
			let candidate = types.iter().find_map(|t| coerce_primitive(current, t));
			if candidate.is_some() {
				next = candidate;
			}
		}
		let current = next.as_ref().unwrap_or(value);
		if types.contains(&"object")
			&& let Some(properties) = current.as_object()
		{
			let coerced = self.coerce_properties(properties, object, &b)?;
			if coerced.is_some() {
				next = coerced;
			}
		}
		let current = next.as_ref().unwrap_or(value);
		if types.contains(&"array")
			&& let Some(items) = current.as_array()
		{
			let coerced = self.coerce_items(items, object, &b)?;
			if coerced.is_some() {
				next = coerced;
			}
		}
		Ok(settle(next, value))
	}

	fn coerce_union(&self, value: &Value, branches: &[Node], base: &str) -> PiResult<Option<Value>> {
		if branches
			.iter()
			.any(|branch| self.passes(branch, base, value) == Some(true))
		{
			return Ok(None);
		}
		for branch in branches {
			let coerced = self.coerce_types(value, branch, base)?;
			if self.passes(branch, base, coerced.as_ref().unwrap_or(value)) == Some(true) {
				return Ok(coerced);
			}
		}
		Ok(None)
	}

	fn coerce_properties(
		&self,
		value: &Map<String, Value>,
		schema: &SchemaObject,
		base: &str,
	) -> PiResult<Option<Value>> {
		let properties = schema.object("properties");
		let patterns = patterns_of(schema)?;
		let additional = schema
			.get("additionalProperties")
			.filter(|additional| additional.as_object().is_some());
		let mut out: Option<Map<String, Value>> = None;
		for (key, property) in properties.into_iter().flat_map(SchemaObject::entries) {
			if let Some(current) = value.get(key) {
				let next = self.coerce_types(current, property, base)?;
				set(&mut out, value, key, next);
			}
		}
		for key in value.keys() {
			let mut matched = properties.is_some_and(|properties| properties.has(key));
			for (pattern, item) in &patterns {
				if !pattern.test(key) {
					continue;
				}
				matched = true;
				let Some(current) = out.as_ref().unwrap_or(value).get(key).cloned() else {
					continue;
				};
				let next = self.coerce_types(&current, item, base)?;
				set(&mut out, value, key, next);
			}
			if !matched && let Some(additional) = additional {
				let next = self.coerce_types(&value[key], additional, base)?;
				set(&mut out, value, key, next);
			}
		}
		Ok(out.map(Value::Object))
	}

	fn coerce_items(&self, value: &[Value], schema: &SchemaObject, base: &str) -> PiResult<Option<Value>> {
		map_items(value, |item, i| match item_schema_at(schema, i) {
			Some(item_schema) if item_schema.as_object().is_some() => self.coerce_types(item, item_schema, base),
			_ => Ok(None),
		})
	}
}

impl Entered {
	fn of(value: &Value) -> Entered {
		match value {
			Value::Array(_) | Value::Object(_) => Entered::Object(value as *const Value as usize),
			_ => Entered::Primitive(value.clone()),
		}
	}

	/// `Object.is(entered, value)`.
	fn is(&self, value: &Value) -> bool {
		match self {
			Entered::Object(address) => *address == value as *const Value as usize,
			Entered::Primitive(Value::Number(a)) => match value.as_f64() {
				Some(b) => {
					let a = a.as_f64().unwrap_or(f64::NAN);
					a == b && a.is_sign_negative() == b.is_sign_negative()
				}
				None => false,
			},
			Entered::Primitive(primitive) => strict_equals(primitive, value),
		}
	}
}

fn memo_key(value: &Value) -> MemoKey {
	match PrimitiveKey::of(value) {
		Some(key) => MemoKey::Primitive(key),
		None => MemoKey::Json(json_stringify(value)),
	}
}

/// `next`, unless it is the same primitive as `value` (`next === value`).
fn settle(next: Option<Value>, value: &Value) -> Option<Value> {
	next.filter(|next| !strict_equals(next, value))
}

/// `out[key] = next` on a copy of `value`, unless `next` is unchanged from the current `(out ?? value)[key]`.
fn set(out: &mut Option<Map<String, Value>>, value: &Map<String, Value>, key: &str, next: Option<Value>) {
	let Some(next) = next else {
		return;
	};
	if out
		.as_ref()
		.unwrap_or(value)
		.get(key)
		.is_some_and(|current| strict_equals(current, &next))
	{
		return;
	}
	out.get_or_insert_with(|| value.clone()).insert(key.to_string(), next);
}

/// The regular expressions of `patternProperties` with their schemas.
fn patterns_of(node: &SchemaObject) -> PiResult<Vec<(Rc<JsRegex>, &Node)>> {
	let Some(patterns) = node.object("patternProperties") else {
		return Ok(Vec::new());
	};
	patterns
		.entries()
		.map(|(pattern, schema)| Ok((shared_regex(pattern)?, schema)))
		.collect()
}

/// The schema of an array's item at `index`: a tuple's (`prefixItems`, or a draft-7 `items` array) for its prefix,
/// `items` after `prefixItems`, or `items` for every item. `None` when nothing applies.
fn item_schema_at(schema: &SchemaObject, index: usize) -> Option<&Node> {
	if let Some(items) = schema.array("items") {
		return items.get(index);
	}
	if let Some(prefix) = schema.array("prefixItems")
		&& index < prefix.len()
	{
		return Some(&prefix[index]);
	}
	schema.get("items")
}

/// `items` mapped by `map`; `None` (the same array) when every item maps to itself.
fn map_items(
	items: &[Value],
	mut map: impl FnMut(&Value, usize) -> PiResult<Option<Value>>,
) -> PiResult<Option<Value>> {
	let mut out: Option<Vec<Value>> = None;
	for (i, item) in items.iter().enumerate() {
		let Some(next) = settle(map(item, i)?, item) else {
			continue;
		};
		out.get_or_insert_with(|| items.to_vec())[i] = next;
	}
	Ok(out.map(Value::Array))
}

// -------------------------------------------------------------------------------------------------------------------
// TypeBox's conversions of one value (value/convert/try in TypeBox 1.3.27): a result, or None when there is none
// -------------------------------------------------------------------------------------------------------------------

/// The JSON value of a number: an integer within the safe range as an integer, so it equals the integer JSON parses.
fn number(x: f64) -> Value {
	if x.fract() == 0.0 && x.abs() <= MAX_SAFE_INTEGER && !(x == 0.0 && x.is_sign_negative()) {
		return Value::from(x as i64);
	}
	serde_json::Number::from_f64(x).map_or(Value::Null, Value::Number)
}

/// A literal (`const`, an enum value) taken from a value of another type that converts to exactly it.
fn convert_literal(literal: &Value, value: &Value) -> Option<Value> {
	if strict_equals(literal, value) {
		return None;
	}
	let result = match literal {
		Value::String(_) => try_string(value),
		Value::Number(_) => try_number(value).map(number),
		Value::Bool(_) => try_boolean(value).map(Value::Bool),
		_ => None,
	};
	result.filter(|result| strict_equals(result, literal))
}

fn try_string(value: &Value) -> Option<Value> {
	match value {
		Value::Bool(b) => Some(Value::String(b.to_string())),
		Value::Number(n) => Some(Value::String(number_to_string(n.as_f64().unwrap_or(f64::NAN)))),
		Value::Null => Some(Value::String("null".into())),
		Value::String(_) => Some(value.clone()),
		_ => None,
	}
}

const MAX_SAFE_INTEGER: f64 = 9_007_199_254_740_991.0;

fn try_number(value: &Value) -> Option<f64> {
	match value {
		Value::Bool(b) => Some(if *b { 1.0 } else { 0.0 }),
		Value::Number(n) => n.as_f64(),
		Value::Null => Some(0.0),
		Value::String(s) => {
			let coerced = string_to_number(s);
			if coerced.is_finite() {
				return Some(coerced);
			}
			let lowercase = s.to_lowercase();
			if lowercase == "false" {
				return Some(0.0);
			}
			if lowercase == "true" {
				return Some(1.0);
			}
			big_int_of_string(s)
				.filter(|big| big.abs() <= MAX_SAFE_INTEGER as i128)
				.map(|big| big as f64)
		}
		_ => None,
	}
}

/// TypeBox's TryBigInt of a string: `12n`, `12.5` (truncated) or `12`; true and false are not reached from try_number.
/// An integer beyond `i128` is out of the safe range either way, so it is `None`.
fn big_int_of_string(value: &str) -> Option<i128> {
	let (sign, digits) = match value.strip_prefix('-') {
		Some(rest) => (-1, rest),
		None => (1, value),
	};
	let integer = |s: &str| {
		s == "0" || (s.starts_with(|c: char| c.is_ascii_digit() && c != '0') && s.bytes().all(|c| c.is_ascii_digit()))
	};
	let whole = if let Some(whole) = digits.strip_suffix('n') {
		whole
	} else if let Some((whole, fraction)) = digits.split_once('.') {
		if fraction.is_empty() || !fraction.bytes().all(|c| c.is_ascii_digit()) {
			return None;
		}
		whole
	} else {
		digits
	};
	if !integer(whole) {
		return None;
	}
	whole.parse::<i128>().ok().map(|n| sign * n)
}

fn try_boolean(value: &Value) -> Option<bool> {
	match value {
		Value::Bool(b) => Some(*b),
		Value::Number(n) => match n.as_f64() {
			Some(0.0) => Some(false),
			Some(1.0) => Some(true),
			_ => None,
		},
		Value::Null => Some(false),
		Value::String(s) => {
			let lowercase = s.to_lowercase();
			if lowercase == "false" || s == "0" {
				return Some(false);
			}
			if lowercase == "true" || s == "1" {
				return Some(true);
			}
			None
		}
		_ => None,
	}
}

fn try_null(value: &Value) -> Option<()> {
	match value {
		Value::Bool(b) => (!b).then_some(()),
		Value::Number(n) => (n.as_f64() == Some(0.0)).then_some(()),
		Value::Null => Some(()),
		Value::String(s) => {
			let lowercase = s.to_lowercase();
			(lowercase == "undefined" || lowercase == "null" || s.is_empty() || s == "0").then_some(())
		}
		_ => None,
	}
}

/// `Number(s)`: JavaScript's StringToNumber (whitespace-trimmed decimal, `0x`/`0o`/`0b` integers, `Infinity`; `""` is 0).
fn string_to_number(s: &str) -> f64 {
	let s = js_trim(s);
	if s.is_empty() {
		return 0.0;
	}
	for (prefix, radix) in [("0x", 16), ("0X", 16), ("0o", 8), ("0O", 8), ("0b", 2), ("0B", 2)] {
		if let Some(digits) = s.strip_prefix(prefix) {
			if digits.is_empty() || !digits.chars().all(|c| c.is_digit(radix)) {
				return f64::NAN;
			}
			return digits.chars().fold(0.0, |n, c| {
				n * f64::from(radix) + f64::from(c.to_digit(radix).unwrap_or(0))
			});
		}
	}
	let unsigned = s.strip_prefix(['+', '-']).unwrap_or(s);
	if unsigned == "Infinity" {
		return if s.starts_with('-') {
			f64::NEG_INFINITY
		} else {
			f64::INFINITY
		};
	}
	if !is_decimal_literal(unsigned) {
		return f64::NAN;
	}
	s.parse::<f64>().unwrap_or(f64::NAN)
}

/// StrUnsignedDecimalLiteral: `1`, `1.`, `.5`, `1.5e-3`.
fn is_decimal_literal(s: &str) -> bool {
	let (mantissa, exponent) = match s.find(['e', 'E']) {
		Some(i) => (&s[..i], Some(&s[i + 1..])),
		None => (s, None),
	};
	let (whole, fraction) = mantissa.split_once('.').unwrap_or((mantissa, ""));
	let digits = |s: &str| s.bytes().all(|c| c.is_ascii_digit());
	if (whole.is_empty() && fraction.is_empty()) || !digits(whole) || !digits(fraction) {
		return false;
	}
	match exponent {
		None => true,
		Some(exponent) => {
			let exponent = exponent.strip_prefix(['+', '-']).unwrap_or(exponent);
			!exponent.is_empty() && digits(exponent)
		}
	}
}

/// `s.trim()`: JavaScript's WhiteSpace and LineTerminator characters.
fn js_trim(s: &str) -> &str {
	s.trim_matches(|c: char| {
		matches!(
			c,
			'\u{9}' | '\u{a}' | '\u{b}' | '\u{c}' | '\u{d}' | ' ' | '\u{a0}' | '\u{1680}' | '\u{2000}'
				..='\u{200a}' | '\u{2028}' | '\u{2029}' | '\u{202f}' | '\u{205f}' | '\u{3000}' | '\u{feff}'
		)
	})
}

// -------------------------------------------------------------------------------------------------------------------
// Primitive coercion by type (pass 3)
// -------------------------------------------------------------------------------------------------------------------

fn types_of(schema: &SchemaObject) -> Vec<&str> {
	match schema.get("type") {
		Some(Node::String(t)) => vec![&**t],
		Some(Node::Array(types)) => types.iter().filter_map(Node::as_str).collect(),
		_ => Vec::new(),
	}
}

fn matches_type(value: &Value, t: &str) -> bool {
	match t {
		"number" => value.is_number(),
		"integer" => is_integer(value),
		"boolean" => value.is_boolean(),
		"string" => value.is_string(),
		"null" => value.is_null(),
		"array" => value.is_array(),
		"object" => value.is_object(),
		_ => false,
	}
}

/// `value` coerced to `t`, `None` when it stays as it is.
fn coerce_primitive(value: &Value, t: &str) -> Option<Value> {
	match t {
		"number" | "integer" => {
			if value.is_null() {
				return Some(Value::from(0));
			}
			if let Some(s) = value.as_str()
				&& !js_trim(s).is_empty()
			{
				let parsed = string_to_number(s);
				let accepted = if t == "number" {
					parsed.is_finite()
				} else {
					parsed.is_finite() && parsed.fract() == 0.0
				};
				if accepted {
					return Some(number(parsed));
				}
			}
			value.as_bool().map(|b| Value::from(if b { 1 } else { 0 }))
		}
		"boolean" => match value {
			Value::Null => Some(Value::Bool(false)),
			Value::String(s) if s == "true" => Some(Value::Bool(true)),
			Value::String(s) if s == "false" => Some(Value::Bool(false)),
			Value::Number(n) if n.as_f64() == Some(1.0) => Some(Value::Bool(true)),
			Value::Number(n) if n.as_f64() == Some(0.0) => Some(Value::Bool(false)),
			_ => None,
		},
		"string" => match value {
			Value::Null => Some(Value::String(String::new())),
			Value::Number(n) => Some(Value::String(number_to_string(n.as_f64().unwrap_or(f64::NAN)))),
			Value::Bool(b) => Some(Value::String(b.to_string())),
			_ => None,
		},
		"null" => match value {
			Value::String(s) if s.is_empty() => Some(Value::Null),
			Value::Number(n) if n.as_f64() == Some(0.0) => Some(Value::Null),
			Value::Bool(false) => Some(Value::Null),
			_ => None,
		},
		_ => None,
	}
}
