//! Port of `packages/schema/src/compiler.ts`.
//! PORT-STATUS: reviewed
//!
//! The compiler: a schema to one check closure, built once. Keywords group by the type they constrain, so a value
//! is classified once per schema, and the common shapes (closed objects, arrays of one item schema, tagged unions,
//! leaf schemas) get one specialized closure each.
//!
//! The TypeScript builds plain closures (no `new Function`), and so does this port: every factory of `checks.rs` and
//! `annotations.rs` returns an `Rc` closure. Two Rust-only rules keep closures from forming reference cycles: the
//! indirection of a recursive reference holds its slot weakly, and a `$dynamicRef` check holds its compiler weakly.
//! The compiler keeps the slots, and `root()` returns the root check bundled with the compiler state whenever a slot
//! or a dynamic lookup exists, so a check stays valid while anything holds it. A schema that a `$dynamicRef` reaches
//! at run time and that fails to compile there makes that check fail, where the TypeScript throws from the check.
//! `RESTART` is a `PiError` named `"RESTART"` instead of a thrown symbol.
//!
//! Accepted difference: below `unevaluatedProperties`/`unevaluatedItems` the TypeScript copies a schema's other
//! keywords with `rest[k] = node[k]`, so an own `"__proto__"` key holding an object (from `JSON.parse`) becomes the
//! copy's prototype and its keywords apply: `{"unevaluatedProperties": true, "__proto__": {"required": ["a"]}}` rejects
//! `{}` there. Here `__proto__` is an unknown keyword on every path, as it is in the TypeScript without `unevaluated*`.

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::rc::{Rc, Weak};

use indexmap::IndexMap;
use pi_host::js::json_stringify;
use pi_host::{PiError, PiResult};
use serde_json::Value;

use crate::annotations::{
	AnnotatingCheck, AnnotatingSlot, annotate_all, annotate_any, annotate_array, annotate_dependent, annotate_if,
	annotate_object, annotating_false, annotating_true, ignore_annotations, indirect_annotating, isolate_annotations,
	scoped_annotating, unevaluated_items, unevaluated_properties, with_fresh_annotations,
};
use crate::checks::{
	CODE, Entry, Guard, INHERITED, PrimitiveKey, REQUIRED, Slot, always, and, any_check, bytes_check, const_check,
	contains_check, declared_object, dependent_required_check, dependent_schemas_check, enum_check, guard_array,
	guard_object, if_check, indirect, is_array, is_object_check, items_check, js_string, leaf_code, length_check,
	list_check, never, not_check, number_check, one_check, open_object, property_count_check, property_names_check,
	scoped_check, string_check, tagged_check, type_check, type_check_of, unique_check,
};
use crate::resolver::{Resolved, Resolver, document_of, resolve_uri};
use crate::shared::shared_regex;
use crate::types::{Check, CompileOptions, Draft, FormatCheck, Node, SchemaObject};

/// Thrown when a compile without dynamic scope tracking meets a bookended `$dynamicRef`: compile again with it.
pub const RESTART: &str = "RESTART";

/// Whether `error` is `RESTART`.
pub fn is_restart(error: &PiError) -> bool {
	error.name == RESTART
}

/// One compile of a schema. Its caches and indexes are garbage once the root check is built, unless the root check
/// needs them (see the file header). Clones share one compile.
#[derive(Clone)]
pub struct Compiler(Rc<State>);

struct State {
	r: Resolver,
	draft: Draft,
	formats: Option<Rc<IndexMap<String, FormatCheck>>>,
	/// Whether checks track the dynamic scope (the schema has a bookended `$dynamicRef`).
	dynamic: bool,
	schema: Node,
	scope: Rc<RefCell<Vec<String>>>,
	cache: RefCell<HashMap<usize, (SchemaObject, Check)>>,
	annotating_cache: RefCell<HashMap<usize, (SchemaObject, AnnotatingCheck)>>,
	slots: RefCell<Vec<Slot>>,
	annotating_slots: RefCell<Vec<AnnotatingSlot>>,
	/// Whether a check holds this compile weakly (a dynamic lookup), so the root check must keep it.
	needed: Cell<bool>,
}

impl Compiler {
	/// `new Compiler(schema, options, dynamic)`.
	pub fn new(schema: Node, options: &CompileOptions, dynamic: bool) -> Compiler {
		let r = Resolver::new(schema.clone(), options.clone());
		let draft = r.draft;
		Compiler(Rc::new(State {
			r,
			draft,
			formats: options.formats.clone(),
			dynamic,
			schema,
			scope: Rc::new(RefCell::new(Vec::new())),
			cache: RefCell::new(HashMap::new()),
			annotating_cache: RefCell::new(HashMap::new()),
			slots: RefCell::new(Vec::new()),
			annotating_slots: RefCell::new(Vec::new()),
			needed: Cell::new(false),
		}))
	}

	/// `compiler.r`.
	pub fn r(&self) -> &Resolver {
		&self.0.r
	}

	/// `compiler.draft`.
	pub fn draft(&self) -> Draft {
		self.0.draft
	}

	/// `compiler.formats`.
	pub fn formats(&self) -> Option<&Rc<IndexMap<String, FormatCheck>>> {
		self.0.formats.as_ref()
	}

	/// `compiler.dynamic`.
	pub fn dynamic(&self) -> bool {
		self.0.dynamic
	}

	/// The check of the root schema.
	pub fn root(&self) -> PiResult<Check> {
		let check = self.check(&self.0.schema, &self.0.r.root_base)?;
		let needed =
			self.0.needed.get() || !self.0.slots.borrow().is_empty() || !self.0.annotating_slots.borrow().is_empty();
		if !needed {
			return Ok(check);
		}
		let state = self.0.clone();
		Ok(Rc::new(move |v| {
			let _keep = &state;
			check(v)
		}))
	}

	/// The check of a schema (cached by schema object; a recursive reference goes through an indirection).
	pub fn check(&self, node: &Node, base: &str) -> PiResult<Check> {
		if node.is_true() {
			return Ok(always());
		}
		if node.is_false() {
			return Ok(never());
		}
		let Some(object) = node.as_object() else {
			return Err(not_a_schema(node));
		};
		if let Some((_, cached)) = self.0.cache.borrow().get(&object.id()) {
			return Ok(cached.clone());
		}
		if let Some(bare) = self.bare_type(object) {
			return Ok(bare);
		}
		let slot: Slot = Rc::new(RefCell::new(None));
		self.0
			.cache
			.borrow_mut()
			.insert(object.id(), (object.clone(), indirect(&slot)));
		self.0.slots.borrow_mut().push(slot.clone());
		let base = self.0.r.base_for(object, base);
		// A schema that fails to compile keeps its empty indirection in the cache, as in the TypeScript.
		let check = self.build(object, &base)?;
		*slot.borrow_mut() = Some(check.clone());
		self.0
			.cache
			.borrow_mut()
			.insert(object.id(), (object.clone(), check.clone()));
		Ok(check)
	}

	fn build(&self, node: &SchemaObject, base: &str) -> PiResult<Check> {
		if self.0.draft == Draft::V7
			&& let Some(reference) = node.str("$ref")
		{
			return self.reference(reference, base);
		}
		if self.0.draft != Draft::V7 && (node.has("unevaluatedProperties") || node.has("unevaluatedItems")) {
			return Ok(with_fresh_annotations(
				self.annotating(&Node::Object(node.clone()), base)?,
			));
		}
		let check = self.plain(node, base)?;
		Ok(if self.0.dynamic && self.0.r.is_resource(node, base) {
			scoped_check(&self.0.scope, base, check)
		} else {
			check
		})
	}

	/// The module-level check of a schema that is only a `type` (with annotations), `ALWAYS` for one with no assertion
	/// at all; `None` for every other schema. These are the most common subschemas and cost nothing to share.
	fn bare_type(&self, node: &SchemaObject) -> Option<Check> {
		let mut r#type = None;
		for (k, v) in node.entries() {
			if k == "type" {
				r#type = Some(v);
			} else if !NEUTRAL_KEYWORDS.contains(&k.as_str()) {
				return None;
			}
		}
		match r#type {
			None => Some(always()),
			Some(t) => t.as_str().and_then(type_check_of),
		}
	}

	/// Every keyword of `node`, not tracking annotations.
	fn plain(&self, node: &SchemaObject, base: &str) -> PiResult<Check> {
		let mut parts: Vec<Check> = Vec::new();
		let types: Option<Vec<Node>> = match node.get("type") {
			None => None,
			Some(Node::Array(list)) => Some(list.to_vec()),
			Some(t) => Some(vec![t.clone()]),
		};
		let single = match &types {
			Some(types) if types.len() == 1 => types[0].as_str(),
			_ => None,
		};
		let type_in_group = if single == Some("bytes") {
			parts.push(bytes_check(
				node.number("minLength").unwrap_or(0.0),
				node.number("maxLength").unwrap_or(f64::INFINITY),
			));
			true
		} else {
			let string = self.string_group(node, single == Some("string"))?;
			let has_string = string.is_some();
			parts.extend(string);
			let numeric = single == Some("number") || single == Some("integer");
			let number = self.number_group(
				node,
				match single {
					Some("number") => 1,
					Some("integer") => 2,
					_ => 0,
				},
			);
			let has_number = number.is_some();
			parts.extend(number);
			let object = self.object_group(node, base, single == Some("object"))?;
			let has_object = object.is_some();
			parts.extend(object);
			let array = self.array_group(node, base, single == Some("array"))?;
			let has_array = array.is_some();
			parts.extend(array);
			(single == Some("string") && has_string)
				|| (numeric && has_number)
				|| (single == Some("object") && has_object)
				|| (single == Some("array") && has_array)
		};
		if let Some(types) = &types
			&& !type_in_group
		{
			parts.insert(0, type_check(types));
		}
		self.in_place(node, base, &mut parts)?;
		Ok(and(parts))
	}

	// The keywords that apply to the value itself, whatever its type.
	fn in_place(&self, node: &SchemaObject, base: &str, parts: &mut Vec<Check>) -> PiResult<()> {
		if let Some(values) = node.array("enum") {
			parts.push(enum_check(&values.iter().map(Node::to_value).collect::<Vec<_>>()));
		}
		if let Some(value) = node.get("const") {
			parts.push(const_check(&value.to_value()));
		}
		if let Some(all_of) = node.array("allOf") {
			let checks = all_of
				.iter()
				.map(|s| self.check(s, base))
				.collect::<PiResult<Vec<_>>>()?;
			parts.push(and(checks));
		}
		if let Some(any_of) = node.array("anyOf") {
			parts.push(self.union(any_of, base, false)?);
		}
		if let Some(one_of) = node.array("oneOf") {
			parts.push(self.union(one_of, base, true)?);
		}
		if let Some(not) = node.get("not") {
			parts.push(not_check(self.check(not, base)?));
		}
		if let Some(cond) = node.get("if")
			&& (node.has("then") || node.has("else"))
		{
			let then = match node.get("then") {
				None => always(),
				Some(then) => self.check(then, base)?,
			};
			let otherwise = match node.get("else") {
				None => always(),
				Some(otherwise) => self.check(otherwise, base)?,
			};
			parts.push(if_check(self.check(cond, base)?, then, otherwise));
		}
		if let Some(reference) = node.str("$ref") {
			parts.push(self.reference(reference, base)?);
		}
		if self.0.draft != Draft::V7
			&& let Some(reference) = node.str("$dynamicRef")
		{
			parts.push(self.dynamic_ref(reference, base)?);
		}
		Ok(())
	}

	fn union(&self, branches: &[Node], base: &str, exactly_one: bool) -> PiResult<Check> {
		if let Some(check) = const_union(branches, exactly_one) {
			return Ok(check);
		}
		if let Some(check) = self.tagged(branches, base)? {
			return Ok(check);
		}
		let checks = branches
			.iter()
			.map(|s| self.check(s, base))
			.collect::<PiResult<Vec<_>>>()?;
		Ok(if exactly_one {
			one_check(checks)
		} else {
			any_check(checks)
		})
	}

	fn reference(&self, reference: &str, base: &str) -> PiResult<Check> {
		let target = self.0.r.resolve(reference, base)?;
		let check = self.check(&target.node, &target.base)?;
		Ok(if self.0.dynamic && target.base != base {
			scoped_check(&self.0.scope, &target.base, check)
		} else {
			check
		})
	}

	fn dynamic_ref(&self, reference: &str, base: &str) -> PiResult<Check> {
		let Some((initial, name)) = self.bookended(reference, base)? else {
			return self.reference(reference, base);
		};
		let fallback = self.check(&initial.node, &initial.base)?;
		let lookup = self.dynamic_lookup(name, |c, node, b| c.check(node, b), never);
		let scope = self.0.scope.clone();
		Ok(Rc::new(move |v| {
			let mut i = 0;
			loop {
				let Some(uri) = scope.borrow().get(i).cloned() else {
					break;
				};
				if let Some(check) = lookup(&uri) {
					return check(v);
				}
				i += 1;
			}
			fallback(v)
		}))
	}

	/// The initial target and anchor name of a `$dynamicRef` that resolves dynamically, or `None` for a plain ref.
	fn bookended(&self, reference: &str, base: &str) -> PiResult<Option<(Resolved, String)>> {
		let initial = self.0.r.resolve(reference, base)?;
		let uri = resolve_uri(reference, base);
		let name = uri.get(document_of(&uri).len() + 1..).unwrap_or("").to_string();
		let anchored = initial
			.node
			.as_object()
			.is_some_and(|node| node.str("$dynamicAnchor") == Some(name.as_str()));
		if name.is_empty() || name.starts_with('/') || !anchored {
			return Ok(None);
		}
		if !self.0.dynamic {
			return Err(PiError::named(RESTART, "restart"));
		}
		Ok(Some((initial, name)))
	}

	// Resolved at run time against the dynamic scope, so a schema with a bookended $dynamicRef keeps its compiler.
	fn dynamic_lookup<T: Clone + 'static>(
		&self,
		name: String,
		compile: fn(&Compiler, &Node, &str) -> PiResult<T>,
		failed: fn() -> T,
	) -> impl Fn(&str) -> Option<T> + 'static {
		self.0.needed.set(true);
		let state: Weak<State> = Rc::downgrade(&self.0);
		let found: RefCell<HashMap<String, Option<T>>> = RefCell::new(HashMap::new());
		move |uri| {
			let key = format!("{uri}#{name}");
			if let Some(check) = found.borrow().get(&key) {
				return check.clone();
			}
			let compiler = Compiler(state.upgrade()?);
			let check = compiler.0.r.dynamic_anchor(&key).map(|target| {
				let base = compiler.0.r.base_of(&target).unwrap_or_else(|| uri.to_string());
				compile(&compiler, &Node::Object(target), &base).unwrap_or_else(|_| failed())
			});
			found.borrow_mut().insert(key, check.clone());
			check
		}
	}

	/// anyOf/oneOf whose branches are all objects with a distinct required const on one key: the tag picks the branch.
	fn tagged(&self, branches: &[Node], base: &str) -> PiResult<Option<Check>> {
		if branches.len() < 2 {
			return Ok(None);
		}
		let mut resolved: Vec<SchemaObject> = Vec::new();
		for branch in branches {
			let Some(object) = branch.as_object() else {
				return Ok(None);
			};
			let reference = match object.str("$ref") {
				Some(reference) if object.len() == 1 => Some(reference),
				_ => None,
			};
			let node = match reference {
				None => branch.clone(),
				Some(reference) => self.0.r.resolve(reference, &self.0.r.base_for(object, base))?.node,
			};
			let Some(node) = node.as_object() else {
				return Ok(None);
			};
			resolved.push(node.clone());
		}
		let Some(first) = resolved[0].object("properties") else {
			return Ok(None);
		};
		for key in first.keys() {
			let tags: Vec<Option<Value>> = resolved.iter().map(|s| tag_of(s, key)).collect();
			if tags.iter().any(Option::is_none) {
				continue;
			}
			let tags: Vec<Value> = tags.into_iter().flatten().collect();
			let distinct: HashSet<PrimitiveKey> = tags.iter().filter_map(PrimitiveKey::of).collect();
			if distinct.len() != tags.len() {
				continue;
			}
			let checks = branches
				.iter()
				.map(|s| self.check(s, base))
				.collect::<PiResult<Vec<_>>>()?;
			return Ok(Some(tagged_check(key.clone(), tags, checks)));
		}
		Ok(None)
	}

	fn string_group(&self, node: &SchemaObject, strict: bool) -> PiResult<Option<Check>> {
		let min = node.number("minLength").unwrap_or(0.0);
		let max = node.number("maxLength").unwrap_or(f64::INFINITY);
		let re = match node.str("pattern") {
			Some(pattern) => Some(shared_regex(pattern)?),
			None => None,
		};
		let format = match (&self.0.formats, node.str("format")) {
			(Some(formats), Some(format)) => formats.get(format).cloned(),
			_ => None,
		};
		if min == 0.0 && max == f64::INFINITY && re.is_none() && format.is_none() {
			return Ok(None);
		}
		Ok(Some(string_check(strict, min, max, re, format)))
	}

	fn number_group(&self, node: &SchemaObject, mode: u8) -> Option<Check> {
		let min = node.number("minimum");
		let max = node.number("maximum");
		let xmin = node.number("exclusiveMinimum");
		let xmax = node.number("exclusiveMaximum");
		let multiple_of = node.number("multipleOf");
		if [min, max, xmin, xmax, multiple_of].iter().all(Option::is_none) {
			return None;
		}
		let inf = f64::INFINITY;
		Some(number_check(
			mode,
			min.unwrap_or(-inf),
			max.unwrap_or(inf),
			xmin.unwrap_or(-inf),
			xmax.unwrap_or(inf),
			multiple_of,
		))
	}

	fn object_group(&self, node: &SchemaObject, base: &str, strict: bool) -> PiResult<Option<Check>> {
		let only_core = OBJECT_EXTRAS.iter().all(|k| !node.has(k));
		// A schema with only the core object keywords gets one closure that also does the type guard.
		let guard = if !only_core {
			Guard::None
		} else if strict {
			Guard::Strict
		} else {
			Guard::Loose
		};
		let core = self.object_core(node, base, guard)?;
		if core.is_some() && only_core {
			return Ok(core);
		}
		let mut parts: Vec<Check> = core.into_iter().collect();
		if let Some(names) = node.get("propertyNames") {
			parts.push(property_names_check(self.check(names, base)?));
		}
		let min_properties = node.number("minProperties");
		let max_properties = node.number("maxProperties");
		if min_properties.is_some() || max_properties.is_some() {
			parts.push(property_count_check(
				min_properties.unwrap_or(0.0),
				max_properties.unwrap_or(f64::INFINITY),
			));
		}
		let mut dependent_required: Vec<(String, Vec<String>)> = Vec::new();
		let mut dependent_schemas: Vec<(String, Check)> = Vec::new();
		if self.0.draft != Draft::V7 {
			for (k, list) in entries(node.get("dependentRequired")) {
				if let Some(list) = list.as_array() {
					dependent_required.push((k.clone(), list.iter().map(js_string).collect()));
				}
			}
			for (k, s) in entries(node.get("dependentSchemas")) {
				dependent_schemas.push((k.clone(), self.check(s, base)?));
			}
		}
		for (k, d) in entries(node.get("dependencies")) {
			match d.as_array() {
				Some(list) => dependent_required.push((k.clone(), list.iter().map(js_string).collect())),
				None => dependent_schemas.push((k.clone(), self.check(d, base)?)),
			}
		}
		if !dependent_required.is_empty() {
			parts.push(dependent_required_check(dependent_required));
		}
		if !dependent_schemas.is_empty() {
			parts.push(dependent_schemas_check(dependent_schemas));
		}
		if parts.is_empty() {
			return Ok(if strict { Some(is_object_check()) } else { None });
		}
		Ok(Some(guard_object(and(parts), strict)))
	}

	fn object_core(&self, node: &SchemaObject, base: &str, guard: Guard) -> PiResult<Option<Check>> {
		let empty = SchemaObject::from_entries(IndexMap::new());
		let properties = node.object("properties").unwrap_or(&empty);
		let required: Vec<String> = node
			.array("required")
			.map(|list| list.iter().filter_map(|k| k.as_str().map(str::to_string)).collect())
			.unwrap_or_default();
		let patterns = node.object("patternProperties").unwrap_or(&empty);
		let additional = node.get("additionalProperties");
		let keys: Vec<String> = properties.keys().cloned().collect();
		let pattern_keys: Vec<String> = patterns.keys().cloned().collect();
		if keys.len() + required.len() + pattern_keys.len() == 0 && additional.is_none_or(Node::is_true) {
			return Ok(None);
		}
		let mut entries: Vec<Entry> = Vec::new();
		for k in &keys {
			let check = self.check(properties.get(k).expect("a key of properties"), base)?;
			let flags = (leaf_code(&check) & CODE)
				| (if required.contains(k) { REQUIRED } else { 0 })
				| (if OBJECT_PROTOTYPE_MEMBERS.contains(&k.as_str()) {
					INHERITED
				} else {
					0
				});
			entries.push((k.clone(), check, flags));
		}
		let extra_required: Vec<String> = required.iter().filter(|k| !properties.has(k)).cloned().collect();
		let mut pats = Vec::new();
		for p in &pattern_keys {
			let re = shared_regex(p)?;
			pats.push((
				re,
				self.check(patterns.get(p).expect("a key of patternProperties"), base)?,
			));
		}
		let extra = match additional {
			None => None,
			Some(additional) if additional.is_true() => None,
			Some(additional) => Some(self.check(additional, base)?),
		};
		let closed = additional.is_some_and(Node::is_false);
		if pats.is_empty() && extra_required.is_empty() && (extra.is_none() || closed) {
			let required_count = keys.iter().filter(|k| required.contains(k)).count();
			return Ok(Some(declared_object(entries, required_count, closed, guard)));
		}
		Ok(Some(open_object(entries, keys, extra_required, pats, extra, guard)))
	}

	fn array_group(&self, node: &SchemaObject, base: &str, strict: bool) -> PiResult<Option<Check>> {
		let mut prefix: &[Node] = &[];
		let rest: Option<&Node>;
		if let Some(items) = node.array("items") {
			prefix = items;
			rest = node.get("additionalItems");
		} else {
			if self.0.draft != Draft::V7
				&& let Some(items) = node.array("prefixItems")
			{
				prefix = items;
			}
			rest = node.get("items");
		}
		let prefix_checks = prefix
			.iter()
			.map(|s| self.check(s, base))
			.collect::<PiResult<Vec<_>>>()?;
		let rest_check = match rest {
			None => None,
			Some(rest) if rest.is_true() => None,
			Some(rest) => Some(self.check(rest, base)?),
		};
		let min_items = node.number("minItems");
		let max_items = node.number("maxItems");
		let unique = matches!(node.get("uniqueItems"), Some(Node::Bool(true)));
		if !node.has("contains") && !unique && (!prefix_checks.is_empty() || rest_check.is_some()) {
			// The common array: one closure for the guard, the length bounds and the items.
			let guard = if strict { Guard::Strict } else { Guard::Loose };
			return Ok(Some(list_check(
				guard,
				min_items.unwrap_or(0.0),
				max_items.unwrap_or(f64::INFINITY),
				prefix_checks,
				rest_check,
			)));
		}
		let mut parts: Vec<Check> = Vec::new();
		if !prefix_checks.is_empty() || rest_check.is_some() {
			parts.push(items_check(prefix_checks, rest_check));
		}
		if min_items.is_some() || max_items.is_some() {
			parts.push(length_check(
				min_items.unwrap_or(0.0),
				max_items.unwrap_or(f64::INFINITY),
			));
		}
		if let Some(contains) = node.get("contains") {
			let draft7 = self.0.draft == Draft::V7;
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
			parts.push(contains_check(self.check(contains, base)?, min, max));
		}
		if unique {
			parts.push(Rc::new(unique_check));
		}
		if parts.is_empty() {
			return Ok(if strict { Some(is_array()) } else { None });
		}
		Ok(Some(guard_array(and(parts), strict)))
	}

	/// The annotating check of a schema evaluated in place (only below `unevaluatedProperties`/`unevaluatedItems`).
	pub fn annotating(&self, node: &Node, base: &str) -> PiResult<AnnotatingCheck> {
		if node.is_true() {
			return Ok(annotating_true());
		}
		if node.is_false() {
			return Ok(annotating_false());
		}
		let Some(object) = node.as_object() else {
			return Err(not_a_schema(node));
		};
		if let Some((_, cached)) = self.0.annotating_cache.borrow().get(&object.id()) {
			return Ok(cached.clone());
		}
		let slot: AnnotatingSlot = Rc::new(RefCell::new(None));
		self.0
			.annotating_cache
			.borrow_mut()
			.insert(object.id(), (object.clone(), indirect_annotating(&slot)));
		self.0.annotating_slots.borrow_mut().push(slot.clone());
		let base = self.0.r.base_for(object, base);
		let check = self.build_annotating(object, &base)?;
		*slot.borrow_mut() = Some(check.clone());
		self.0
			.annotating_cache
			.borrow_mut()
			.insert(object.id(), (object.clone(), check.clone()));
		Ok(check)
	}

	fn build_annotating(&self, node: &SchemaObject, base: &str) -> PiResult<AnnotatingCheck> {
		let mut rest: IndexMap<String, Node> = IndexMap::new();
		let mut annotates = false;
		for (k, v) in node.entries() {
			if ANNOTATING.contains(&k.as_str()) {
				annotates = true;
			} else {
				rest.insert(k.clone(), v.clone());
			}
		}
		let plain = self.plain(&SchemaObject::from_entries(rest), base)?;
		let mut parts: Vec<AnnotatingCheck> = Vec::new();
		if annotates {
			self.annotating_parts(node, base, &mut parts)?;
		}
		let all = if parts.is_empty() {
			ignore_annotations(plain)
		} else {
			annotate_all(plain, parts)
		};
		let own = if node.has("unevaluatedProperties") || node.has("unevaluatedItems") {
			isolate_annotations(all)
		} else {
			all
		};
		Ok(if self.0.dynamic && self.0.r.is_resource(node, base) {
			scoped_annotating(&self.0.scope, base, own)
		} else {
			own
		})
	}

	fn annotating_parts(&self, node: &SchemaObject, base: &str, parts: &mut Vec<AnnotatingCheck>) -> PiResult<()> {
		if node.has("properties") || node.has("patternProperties") || node.has("additionalProperties") {
			let mut props = Vec::new();
			for (k, s) in entries(node.get("properties")) {
				props.push((k.clone(), self.check(s, base)?));
			}
			let mut pats = Vec::new();
			for (p, s) in entries(node.get("patternProperties")) {
				let re = shared_regex(p)?;
				pats.push((re, self.check(s, base)?));
			}
			let extra = match node.get("additionalProperties") {
				None => None,
				Some(additional) => Some(self.check(additional, base)?),
			};
			parts.push(annotate_object(props, pats, extra));
		}
		if node.has("prefixItems") || node.has("items") || node.has("contains") {
			let prefix: &[Node] = node.array("prefixItems").or_else(|| node.array("items")).unwrap_or(&[]);
			let rest = if node.array("items").is_some() {
				node.get("additionalItems")
			} else {
				node.get("items")
			};
			let prefix_checks = prefix
				.iter()
				.map(|s| self.check(s, base))
				.collect::<PiResult<Vec<_>>>()?;
			let rest_check = match rest {
				None => None,
				Some(rest) => Some(self.check(rest, base)?),
			};
			let contains = match node.get("contains") {
				None => None,
				Some(contains) => Some(self.check(contains, base)?),
			};
			parts.push(annotate_array(
				prefix_checks,
				rest_check,
				contains,
				node.number("minContains").unwrap_or(1.0),
				node.number("maxContains").unwrap_or(f64::INFINITY),
			));
		}
		if let Some(all_of) = node.array("allOf") {
			for s in all_of {
				parts.push(self.annotating(s, base)?);
			}
		}
		if let Some(any_of) = node.array("anyOf") {
			let branches = any_of
				.iter()
				.map(|s| self.annotating(s, base))
				.collect::<PiResult<Vec<_>>>()?;
			parts.push(annotate_any(branches, false));
		}
		if let Some(one_of) = node.array("oneOf") {
			let branches = one_of
				.iter()
				.map(|s| self.annotating(s, base))
				.collect::<PiResult<Vec<_>>>()?;
			parts.push(annotate_any(branches, true));
		}
		if let Some(cond) = node.get("if") {
			let then = match node.get("then") {
				None => None,
				Some(then) => Some(self.annotating(then, base)?),
			};
			let otherwise = match node.get("else") {
				None => None,
				Some(otherwise) => Some(self.annotating(otherwise, base)?),
			};
			parts.push(annotate_if(self.annotating(cond, base)?, then, otherwise));
		}
		if let Some(reference) = node.str("$ref") {
			parts.push(self.annotating_ref(reference, base)?);
		}
		if let Some(reference) = node.str("$dynamicRef") {
			parts.push(self.annotating_dynamic_ref(reference, base)?);
		}
		for (k, s) in entries(node.get("dependentSchemas")) {
			parts.push(annotate_dependent(k.clone(), self.annotating(s, base)?));
		}
		if let Some(unevaluated) = node.get("unevaluatedProperties") {
			parts.push(unevaluated_properties(self.check(unevaluated, base)?));
		}
		if let Some(unevaluated) = node.get("unevaluatedItems") {
			parts.push(unevaluated_items(self.check(unevaluated, base)?));
		}
		Ok(())
	}

	fn annotating_ref(&self, reference: &str, base: &str) -> PiResult<AnnotatingCheck> {
		let target = self.0.r.resolve(reference, base)?;
		let check = self.annotating(&target.node, &target.base)?;
		Ok(if self.0.dynamic && target.base != base {
			scoped_annotating(&self.0.scope, &target.base, check)
		} else {
			check
		})
	}

	fn annotating_dynamic_ref(&self, reference: &str, base: &str) -> PiResult<AnnotatingCheck> {
		let Some((initial, name)) = self.bookended(reference, base)? else {
			return self.annotating_ref(reference, base);
		};
		let fallback = self.annotating(&initial.node, &initial.base)?;
		let lookup = self.dynamic_lookup(name, |c, node, b| c.annotating(node, b), annotating_false);
		let scope = self.0.scope.clone();
		Ok(Rc::new(move |v, a| {
			let mut i = 0;
			loop {
				let Some(uri) = scope.borrow().get(i).cloned() else {
					break;
				};
				if let Some(check) = lookup(&uri) {
					return check(v, a);
				}
				i += 1;
			}
			fallback(v, a)
		}))
	}
}

fn not_a_schema(node: &Node) -> PiError {
	PiError::new(format!(
		"A schema must be an object or a boolean, got {}",
		json_stringify(&node.to_value())
	))
}

/// Keywords that never affect a check.
pub const NEUTRAL_KEYWORDS: [&str; 16] = [
	"$schema",
	"$comment",
	"title",
	"description",
	"default",
	"examples",
	"deprecated",
	"readOnly",
	"writeOnly",
	"$defs",
	"definitions",
	"contentEncoding",
	"contentMediaType",
	"minContains",
	"maxContains",
	"$vocabulary",
];

// The object keywords besides the core (properties, required, patternProperties, additionalProperties).
const OBJECT_EXTRAS: [&str; 6] = [
	"propertyNames",
	"minProperties",
	"maxProperties",
	"dependentRequired",
	"dependentSchemas",
	"dependencies",
];

// The keywords an annotating check handles itself; every other keyword compiles into its plain part.
const ANNOTATING: [&str; 20] = [
	"properties",
	"patternProperties",
	"additionalProperties",
	"prefixItems",
	"items",
	"additionalItems",
	"contains",
	"minContains",
	"maxContains",
	"allOf",
	"anyOf",
	"oneOf",
	"if",
	"then",
	"else",
	"$ref",
	"$dynamicRef",
	"dependentSchemas",
	"unevaluatedProperties",
	"unevaluatedItems",
];

// The names `k in Object.prototype` is true for.
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

/// anyOf/oneOf whose branches are all `{ const: primitive }`: an enum (distinct values make oneOf the same).
fn const_union(branches: &[Node], exactly_one: bool) -> Option<Check> {
	let mut values: Vec<Value> = Vec::new();
	for branch in branches {
		let object = branch.as_object()?;
		let value = object.get("const")?;
		for k in object.keys() {
			if k != "const" && !NEUTRAL_KEYWORDS.contains(&k.as_str()) {
				return None;
			}
		}
		if value.is_object_like() {
			return None;
		}
		let value = value.to_value();
		if exactly_one && values.iter().any(|x| PrimitiveKey::of(x) == PrimitiveKey::of(&value)) {
			return None;
		}
		values.push(value);
	}
	Some(enum_check(&values))
}

// The tag of a tagged-union branch: a primitive const on a required property of an object schema.
fn tag_of(branch: &SchemaObject, key: &str) -> Option<Value> {
	if branch.str("type") != Some("object") {
		return None;
	}
	let required = branch.array("required")?;
	if !required.iter().any(|k| k.as_str() == Some(key)) {
		return None;
	}
	let tag = branch.object("properties")?.object(key)?;
	let value = tag.get("const")?;
	if value.is_object_like() {
		return None;
	}
	Some(value.to_value())
}

// `Object.entries(value)` of a schema object, none for anything else.
fn entries(value: Option<&Node>) -> Vec<(&String, &Node)> {
	match value.and_then(Node::as_object) {
		Some(object) => object.entries().collect(),
		None => Vec::new(),
	}
}
