//! Port of `packages/schema/src/explain.ts`.
//! PORT-STATUS: ported
//!
//! Errors of a failed value: a walk over the schema that reports each failing keyword at the value it failed on.
//! It runs only for values that failed the check, using the compiler's checks for subschemas.
//!
//! The memo of `$ref` targets keys values by their address in the value being explained, as the TypeScript keys
//! them by object identity (a value shared at two places of a JavaScript object is two values in JSON).

use std::cell::RefCell;
use std::collections::HashMap;

use indexmap::IndexMap;
use pi_host::PiResult;
use pi_host::js::{json_stringify, number_to_string, utf16_len};
use serde_json::{Map, Value};

use crate::annotations::new_annotations;
use crate::checks::{
	PrimitiveKey, code_points, const_check, contains_check, enum_check, is_integer, is_multiple, is_object, js_join,
	js_string, never, strict_equals, type_check_of, unique_check,
};
use crate::compiler::Compiler;
use crate::shared::shared_regex;
use crate::types::{Draft, Node, SchemaObject, ValidationError};

type Memo = HashMap<usize, HashMap<usize, (String, Vec<ValidationError>)>>;

/// Explains failures of one compiled schema.
pub struct Explainer {
	c: Compiler,
	schema: Node,
	/// The errors of `$ref` targets for object values, within one errors() call: a recursive union explains each branch
	/// to pick the one to blame, and every branch would explain the same children again, exponentially in the depth.
	memo: RefCell<Option<Memo>>,
}

impl Explainer {
	/// `new Explainer(compiler, schema)`.
	pub fn new(compiler: Compiler, schema: Node) -> PiResult<Explainer> {
		compiler.root()?;
		Ok(Explainer {
			c: compiler,
			schema,
			memo: RefCell::new(None),
		})
	}

	/// The errors of `value`, at least one (`value` failed the check).
	pub fn errors(&self, value: &Value) -> PiResult<Vec<ValidationError>> {
		let mut out = Vec::new();
		let walked = self.walk(&self.schema, &self.c.r().root_base, value, "", &mut out);
		*self.memo.borrow_mut() = None;
		walked?;
		if out.is_empty() {
			push(&mut out, "", "schema", "invalid".into());
		}
		Ok(out)
	}

	fn ok(&self, node: &Node, base: &str, v: &Value) -> PiResult<bool> {
		Ok(self.c.check(node, base)?(v))
	}

	// For $dynamicRef schemas this check runs outside the original dynamic scope, so their errors are approximate.
	fn walk(&self, node: &Node, base: &str, v: &Value, path: &str, out: &mut Vec<ValidationError>) -> PiResult<()> {
		if node.is_true() {
			return Ok(());
		}
		if node.is_false() {
			push(out, path, "false", "no value is allowed here".into());
			return Ok(());
		}
		let Some(object) = node.as_object() else {
			return Ok(());
		};
		let b = self.c.r().base_for(object, base);
		if self.ok(node, &b, v)? {
			return Ok(());
		}
		let start = out.len();
		if self.c.draft() == Draft::V7
			&& let Some(reference) = object.str("$ref")
		{
			return self.walk_ref(reference, &b, v, path, out);
		}
		if let Some(t) = object.get("type") {
			let types: Vec<Node> = match t {
				Node::Array(list) => list.to_vec(),
				t => vec![t.clone()],
			};
			let passes = types
				.iter()
				.any(|t| t.as_str().and_then(type_check_of).unwrap_or_else(never)(v));
			if !passes {
				push(
					out,
					path,
					"type",
					format!("expected {}, got {}", js_join(&types, " or "), type_name(v)),
				);
				return Ok(());
			}
		}
		if let Some(values) = object.array("enum")
			&& !enum_check(&values.iter().map(Node::to_value).collect::<Vec<_>>())(v)
		{
			push(out, path, "enum", format!("expected one of {}", list_values(values)));
		}
		if let Some(value) = object.get("const")
			&& !const_check(&value.to_value())(v)
		{
			push(
				out,
				path,
				"const",
				format!("expected {}", json_stringify(&value.to_value())),
			);
		}
		if let Some(s) = v.as_str()
			&& object.str("type") != Some("bytes")
		{
			self.walk_string(object, s, path, out)?;
		}
		if let Some(n) = v.as_f64() {
			walk_number(object, n, path, out);
		}
		if let Some(o) = v.as_object() {
			self.walk_object(object, &b, v, o, path, out)?;
		}
		if let Some(list) = v.as_array() {
			self.walk_array(object, &b, v, list, path, out)?;
		}
		if let Some(all_of) = object.array("allOf") {
			for s in all_of {
				self.walk(s, &b, v, path, out)?;
			}
		}
		if let Some(any_of) = object.array("anyOf") {
			self.walk_union(any_of, &b, v, path, out, false)?;
		}
		if let Some(one_of) = object.array("oneOf") {
			self.walk_union(one_of, &b, v, path, out, true)?;
		}
		if let Some(not) = object.get("not")
			&& self.ok(not, &b, v)?
		{
			push(out, path, "not", "must not match the schema in not".into());
		}
		if let Some(cond) = object.get("if") {
			let branch = if self.ok(cond, &b, v)? {
				object.get("then")
			} else {
				object.get("else")
			};
			if let Some(branch) = branch {
				self.walk(branch, &b, v, path, out)?;
			}
		}
		if let Some(reference) = object.str("$ref") {
			self.walk_ref(reference, &b, v, path, out)?;
		}
		if out.len() == start && (object.has("unevaluatedProperties") || object.has("unevaluatedItems")) {
			self.walk_unevaluated(object, &b, v, path, out)?;
		}
		if out.len() == start && object.str("$dynamicRef").is_some() {
			push(
				out,
				path,
				"$dynamicRef",
				"does not match the dynamically referenced schema".into(),
			);
		}
		Ok(())
	}

	fn walk_ref(
		&self,
		reference: &str,
		base: &str,
		v: &Value,
		path: &str,
		out: &mut Vec<ValidationError>,
	) -> PiResult<()> {
		let target = self.c.r().resolve(reference, base)?;
		if target.node.is_false() && self.c.r().is_unresolved(reference, base) {
			push(out, path, "$ref", format!("unresolved reference {reference}"));
			return Ok(());
		}
		let Some(node) = target.node.as_object().filter(|_| v.is_object() || v.is_array()) else {
			return self.walk(&target.node, &target.base, v, path, out);
		};
		let value_id = v as *const Value as usize;
		let known = self
			.memo
			.borrow()
			.as_ref()
			.and_then(|memo| memo.get(&node.id()))
			.and_then(|values| values.get(&value_id))
			.filter(|(known, _)| known == path)
			.map(|(_, errors)| errors.clone());
		if let Some(errors) = known {
			out.extend(errors);
			return Ok(());
		}
		let mut errors = Vec::new();
		self.walk(&target.node, &target.base, v, path, &mut errors)?;
		self.memo
			.borrow_mut()
			.get_or_insert_with(HashMap::new)
			.entry(node.id())
			.or_default()
			.insert(value_id, (path.to_string(), errors.clone()));
		out.extend(errors);
		Ok(())
	}

	fn walk_string(&self, node: &SchemaObject, v: &str, path: &str, out: &mut Vec<ValidationError>) -> PiResult<()> {
		let n = code_points(v) as f64;
		if let Some(min) = node.number("minLength")
			&& n < min
		{
			push(
				out,
				path,
				"minLength",
				format!("expected at least {} characters", number_to_string(min)),
			);
		}
		if let Some(max) = node.number("maxLength")
			&& n > max
		{
			push(
				out,
				path,
				"maxLength",
				format!("expected at most {} characters", number_to_string(max)),
			);
		}
		if let Some(pattern) = node.str("pattern")
			&& !shared_regex(pattern)?.test(v)
		{
			push(out, path, "pattern", format!("expected to match {pattern}"));
		}
		let format = match (self.c.formats(), node.str("format")) {
			(Some(formats), Some(format)) => formats.get(format).map(|check| (format, check.clone())),
			_ => None,
		};
		if let Some((name, format)) = format
			&& !format(v)
		{
			push(out, path, "format", format!("expected format {name}"));
		}
		Ok(())
	}

	fn walk_object(
		&self,
		node: &SchemaObject,
		b: &str,
		v: &Value,
		o: &Map<String, Value>,
		path: &str,
		out: &mut Vec<ValidationError>,
	) -> PiResult<()> {
		let empty = SchemaObject::from_entries(IndexMap::new());
		let properties = node.object("properties").unwrap_or(&empty);
		let required: &[Node] = node.array("required").unwrap_or(&[]);
		for k in required {
			if let Some(k) = k.as_str()
				&& !o.contains_key(k)
			{
				push(out, &pointer(path, k), "required", "missing".into());
			}
		}
		for (k, s) in properties.entries() {
			if let Some(x) = o.get(k) {
				self.walk(s, b, x, &pointer(path, k), out)?;
			}
		}
		let patterns: Vec<(&String, &Node)> = node
			.object("patternProperties")
			.map(|p| p.entries().collect())
			.unwrap_or_default();
		for (k, x) in o {
			let mut matched = properties.has(k);
			for (p, s) in &patterns {
				if shared_regex(p)?.test(k) {
					matched = true;
					self.walk(s, b, x, &pointer(path, k), out)?;
				}
			}
			if !matched && let Some(additional) = node.get("additionalProperties") {
				if additional.is_false() {
					push(out, &pointer(path, k), "additionalProperties", "unknown field".into());
				} else {
					self.walk(additional, b, x, &pointer(path, k), out)?;
				}
			}
			if let Some(names) = node.get("propertyNames")
				&& !self.ok(names, b, &Value::String(k.clone()))?
			{
				push(out, &pointer(path, k), "propertyNames", "invalid property name".into());
			}
		}
		let count = o.len() as f64;
		if let Some(min) = node.number("minProperties")
			&& count < min
		{
			push(
				out,
				path,
				"minProperties",
				format!("expected at least {} properties", number_to_string(min)),
			);
		}
		if let Some(max) = node.number("maxProperties")
			&& count > max
		{
			push(
				out,
				path,
				"maxProperties",
				format!("expected at most {} properties", number_to_string(max)),
			);
		}
		let mut dependencies: IndexMap<String, Node> = IndexMap::new();
		if self.c.draft() != Draft::V7
			&& let Some(dependent_required) = node.object("dependentRequired")
		{
			for (k, d) in dependent_required.entries() {
				dependencies.insert(k.clone(), d.clone());
			}
		}
		if let Some(deps) = node.object("dependencies") {
			for (k, d) in deps.entries() {
				dependencies.insert(k.clone(), d.clone());
			}
		}
		for (k, d) in &dependencies {
			if !o.contains_key(k) {
				continue;
			}
			match d.as_array() {
				None => self.walk(d, b, v, path, out)?,
				Some(list) => {
					for r in list {
						let r = js_string(r);
						if !o.contains_key(&r) {
							push(
								out,
								&pointer(path, &r),
								"dependentRequired",
								format!("required when {k} is present"),
							);
						}
					}
				}
			}
		}
		if self.c.draft() != Draft::V7
			&& let Some(dependent_schemas) = node.object("dependentSchemas")
		{
			for (k, s) in dependent_schemas.entries() {
				if o.contains_key(k) {
					self.walk(s, b, v, path, out)?;
				}
			}
		}
		Ok(())
	}

	fn walk_array(
		&self,
		node: &SchemaObject,
		b: &str,
		v: &Value,
		list: &[Value],
		path: &str,
		out: &mut Vec<ValidationError>,
	) -> PiResult<()> {
		let mut prefix: &[Node] = &[];
		let rest: Option<&Node>;
		if let Some(items) = node.array("items") {
			prefix = items;
			rest = node.get("additionalItems");
		} else {
			if self.c.draft() != Draft::V7
				&& let Some(items) = node.array("prefixItems")
			{
				prefix = items;
			}
			rest = node.get("items");
		}
		for (i, x) in list.iter().enumerate() {
			let s = if i < prefix.len() { Some(&prefix[i]) } else { rest };
			if let Some(s) = s {
				self.walk(s, b, x, &format!("{path}/{i}"), out)?;
			}
		}
		let length = list.len() as f64;
		if let Some(min) = node.number("minItems")
			&& length < min
		{
			push(
				out,
				path,
				"minItems",
				format!("expected at least {} items", number_to_string(min)),
			);
		}
		if let Some(max) = node.number("maxItems")
			&& length > max
		{
			push(
				out,
				path,
				"maxItems",
				format!("expected at most {} items", number_to_string(max)),
			);
		}
		if matches!(node.get("uniqueItems"), Some(Node::Bool(true))) && !unique_check(v) {
			push(out, path, "uniqueItems", "expected unique items".into());
		}
		if let Some(contains) = node.get("contains") {
			let draft7 = self.c.draft() == Draft::V7;
			let min = if draft7 {
				1.0
			} else {
				node.number("minContains").unwrap_or(1.0)
			};
			let max = if draft7 {
				f64::INFINITY
			} else {
				node.number("maxContains").unwrap_or(f64::INFINITY)
			};
			if !contains_check(self.c.check(contains, b)?, min, max)(v) {
				push(out, path, "contains", contains_message(min, max));
			}
		}
		Ok(())
	}

	fn walk_union(
		&self,
		branches: &[Node],
		b: &str,
		v: &Value,
		path: &str,
		out: &mut Vec<ValidationError>,
		exactly_one: bool,
	) -> PiResult<()> {
		let mut matches = 0;
		for s in branches {
			if self.ok(s, b, v)? {
				matches += 1;
			}
		}
		if matches == 1 || (!exactly_one && matches > 0) {
			return Ok(());
		}
		let keyword = if exactly_one { "oneOf" } else { "anyOf" };
		if matches > 1 {
			push(out, path, keyword, "matches more than one alternative".into());
			return Ok(());
		}
		// A tagged union reports the branch its tag selects; otherwise the branch whose first error is deepest.
		let tag = self.tag_of(branches, b)?;
		if let Some((key, values)) = tag
			&& let Some(o) = v.as_object()
		{
			let i = o
				.get(&key)
				.and_then(|x| values.iter().position(|value| strict_equals(value, x)));
			match i {
				None => {
					let listed: Vec<Node> = values.iter().map(Node::from_value).collect();
					push(
						out,
						&pointer(path, &key),
						keyword,
						format!("expected one of {}", list_values(&listed)),
					);
				}
				Some(i) => self.walk(&branches[i], b, v, path, out)?,
			}
			return Ok(());
		}
		// An object is blamed on the branch that declares most of its properties (the one it was most likely meant for),
		// then on the branch whose first error is deepest.
		let mut best: Option<(Vec<ValidationError>, usize)> = None;
		for s in branches {
			let mut errors = Vec::new();
			self.walk(s, b, v, path, &mut errors)?;
			if errors.is_empty() {
				continue;
			}
			let declared = match v.as_object() {
				Some(o) => self.declared_keys(s, b, o)?,
				None => 0,
			};
			let better = match &best {
				None => true,
				Some((best_errors, best_declared)) => {
					declared > *best_declared
						|| (declared == *best_declared && utf16_len(&errors[0].path) > utf16_len(&best_errors[0].path))
				}
			};
			if better {
				best = Some((errors, declared));
			}
		}
		match best {
			Some((errors, _)) => out.extend(errors),
			None => push(out, path, keyword, "matches no alternative".into()),
		}
		Ok(())
	}

	/// How many of the keys of `v` the branch `s` (through a `$ref`) declares in `properties`.
	fn declared_keys(&self, s: &Node, b: &str, v: &Map<String, Value>) -> PiResult<usize> {
		let node = self.through_ref(s, b)?;
		let Some(properties) = node.as_object().and_then(|node| node.object("properties")) else {
			return Ok(0);
		};
		Ok(v.keys().filter(|k| properties.has(k)).count())
	}

	// `s`, or the schema its `$ref` names.
	fn through_ref(&self, s: &Node, b: &str) -> PiResult<Node> {
		match s.as_object().and_then(|s| s.str("$ref")) {
			Some(reference) => Ok(self.c.r().resolve(reference, b)?.node),
			None => Ok(s.clone()),
		}
	}

	fn tag_of(&self, branches: &[Node], b: &str) -> PiResult<Option<(String, Vec<Value>)>> {
		let resolved = branches
			.iter()
			.map(|s| self.through_ref(s, b))
			.collect::<PiResult<Vec<_>>>()?;
		let Some(first) = resolved
			.first()
			.and_then(Node::as_object)
			.and_then(|first| first.object("properties"))
		else {
			return Ok(None);
		};
		for key in first.keys() {
			let values: Vec<Option<Value>> = resolved
				.iter()
				.map(|s| {
					let tag = s
						.as_object()
						.and_then(|s| s.object("properties"))
						.and_then(|p| p.object(key));
					tag.and_then(|tag| tag.get("const")).map(Node::to_value)
				})
				.collect();
			let primitive = values
				.iter()
				.all(|x| x.as_ref().is_some_and(|x| !x.is_object() && !x.is_array()));
			if !primitive {
				continue;
			}
			let values: Vec<Value> = values.into_iter().flatten().collect();
			let distinct: std::collections::HashSet<PrimitiveKey> =
				values.iter().filter_map(PrimitiveKey::of).collect();
			if distinct.len() == values.len() {
				return Ok(Some((key.clone(), values)));
			}
		}
		Ok(None)
	}

	fn walk_unevaluated(
		&self,
		node: &SchemaObject,
		b: &str,
		v: &Value,
		path: &str,
		out: &mut Vec<ValidationError>,
	) -> PiResult<()> {
		let unevaluated_properties = node.get("unevaluatedProperties");
		let unevaluated_items = node.get("unevaluatedItems");
		let rest: IndexMap<String, Node> = node
			.entries()
			.filter(|(k, _)| *k != "unevaluatedProperties" && *k != "unevaluatedItems")
			.map(|(k, s)| (k.clone(), s.clone()))
			.collect();
		let mut a = new_annotations();
		self.c.annotating(&Node::Object(SchemaObject::from_entries(rest)), b)?(v, &mut a);
		if let Some(o) = v.as_object()
			&& is_object(v)
			&& let Some(unevaluated) = unevaluated_properties
			&& !a.all_props
		{
			for (k, x) in o {
				if a.props.as_ref().is_some_and(|props| props.contains(k)) {
					continue;
				}
				if unevaluated.is_false() {
					push(out, &pointer(path, k), "unevaluatedProperties", "unknown field".into());
				} else {
					self.walk(unevaluated, b, x, &pointer(path, k), out)?;
				}
			}
		}
		if let Some(list) = v.as_array()
			&& let Some(unevaluated) = unevaluated_items
			&& !a.all_items
		{
			for (i, x) in list.iter().enumerate().skip(a.items) {
				if a.indexes.as_ref().is_some_and(|indexes| indexes.contains(&i)) {
					continue;
				}
				if unevaluated.is_false() {
					push(
						out,
						&format!("{path}/{i}"),
						"unevaluatedItems",
						"unexpected item".into(),
					);
				} else {
					self.walk(unevaluated, b, x, &format!("{path}/{i}"), out)?;
				}
			}
		}
		Ok(())
	}
}

fn push(out: &mut Vec<ValidationError>, path: &str, keyword: &str, message: String) {
	out.push(ValidationError {
		path: path.to_string(),
		keyword: keyword.to_string(),
		message,
	});
}

fn walk_number(node: &SchemaObject, v: f64, path: &str, out: &mut Vec<ValidationError>) {
	if let Some(min) = node.number("minimum")
		&& v < min
	{
		push(out, path, "minimum", format!("expected >= {}", number_to_string(min)));
	}
	if let Some(max) = node.number("maximum")
		&& v > max
	{
		push(out, path, "maximum", format!("expected <= {}", number_to_string(max)));
	}
	if let Some(xmin) = node.number("exclusiveMinimum")
		&& v <= xmin
	{
		push(
			out,
			path,
			"exclusiveMinimum",
			format!("expected > {}", number_to_string(xmin)),
		);
	}
	if let Some(xmax) = node.number("exclusiveMaximum")
		&& v >= xmax
	{
		push(
			out,
			path,
			"exclusiveMaximum",
			format!("expected < {}", number_to_string(xmax)),
		);
	}
	if let Some(d) = node.number("multipleOf")
		&& !is_multiple(v, d)
	{
		push(
			out,
			path,
			"multipleOf",
			format!("expected a multiple of {}", number_to_string(d)),
		);
	}
}

// Enum errors name the allowed values; a long list (an enum of thousands) is cut, so the message stays readable.
const LISTED_VALUES: usize = 20;

/// `values` as JSON, at most LISTED_VALUES of them, then how many more there are.
fn list_values(values: &[Node]) -> String {
	let json = |values: &[Node]| json_stringify(&Value::Array(values.iter().map(Node::to_value).collect()));
	if values.len() <= LISTED_VALUES {
		return json(values);
	}
	let listed = json(&values[..LISTED_VALUES]);
	format!(
		"{}, ... and {} more]",
		&listed[..listed.len() - 1],
		values.len() - LISTED_VALUES
	)
}

fn contains_message(min: f64, max: f64) -> String {
	if max == f64::INFINITY {
		return format!(
			"expected at least {} matching {}",
			number_to_string(min),
			if min == 1.0 { "item" } else { "items" }
		);
	}
	if min == 0.0 {
		return format!(
			"expected at most {} matching {}",
			number_to_string(max),
			if max == 1.0 { "item" } else { "items" }
		);
	}
	format!(
		"expected between {} and {} matching items",
		number_to_string(min),
		number_to_string(max)
	)
}

fn pointer(path: &str, key: &str) -> String {
	format!("{path}/{}", key.replace('~', "~0").replace('/', "~1"))
}

fn type_name(v: &Value) -> &'static str {
	match v {
		Value::Null => "null",
		Value::Array(_) => "array",
		Value::Number(_) if is_integer(v) => "integer",
		Value::Number(_) => "number",
		Value::String(_) => "string",
		Value::Bool(_) => "boolean",
		Value::Object(_) => "object",
	}
}
