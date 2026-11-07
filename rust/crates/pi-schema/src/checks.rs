//! Port of `packages/schema/src/checks.ts`.
//! PORT-STATUS: reviewed
//!
//! Check factories: each returns a closure that captures only its arguments, never the compiler, so a compiled
//! schema keeps no compile-time state alive. Leaf checks are module constants (per-thread singletons here, so their
//! identity can be compared as the TypeScript compares the function objects) that object and array loops inline.
//!
//! Values are `serde_json::Value`s, so the JavaScript-only cases have no counterpart: there is no `Uint8Array` (the
//! `bytes` type matches no value), no `undefined` property, and no inherited `Object.prototype` member to guard
//! against (`INHERITED` is kept as a flag but every lookup is an own lookup). Numbers compare as `f64`, like `===`.
//! The JavaScript helpers the factories need (`===` on values, `String(x)` of a schema value) are at the end.
//!
//! Rust-only: checks return a bool, but two TypeScript checks throw out of the whole call: a reference that recurses
//! without end (`{"$ref": "#"}`) runs out of stack (a RangeError), and the indirection of a subschema that failed to
//! compile calls its empty slot (a TypeError). Here such a check fails and records the error, and [`catching`] returns
//! it to the callers that can throw.
//!
//! Accepted differences:
//! - [`js_string`] gives "[object Object]" for every object. In JavaScript, converting an object with an own `toString`
//!   key from JSON throws a TypeError, so `{"dependentRequired": {"a": [{"toString": 1}]}}` (not a valid schema: the
//!   entries must be strings) throws on `{"a": 1}` there and fails here.
//! - A throw does not skip the `$dynamicRef` scope pop. In TypeScript, a RangeError inside a scoped check leaves that
//!   resource's URI on the scope for good, so later checks on the same validator can resolve `$dynamicRef` to it.

use std::cell::RefCell;
use std::collections::HashSet;
use std::rc::{Rc, Weak};

use pi_host::js::number_to_string;
use pi_host::{PiError, PiResult};
use serde_json::{Map, Value};

use crate::shared::JsRegex;
use crate::types::{Check, FormatCheck, Node};

struct Leaves {
	always: Check,
	never: Check,
	is_string: Check,
	is_number: Check,
	is_integer: Check,
	is_boolean: Check,
	is_null: Check,
	is_array: Check,
	is_bytes: Check,
	is_object: Check,
}

thread_local! {
	static LEAVES: Leaves = Leaves {
		always: Rc::new(|_| true),
		never: Rc::new(|_| false),
		is_string: Rc::new(|v| v.is_string()),
		is_number: Rc::new(|v| v.is_number()),
		is_integer: Rc::new(is_integer),
		is_boolean: Rc::new(|v| v.is_boolean()),
		is_null: Rc::new(|v| v.is_null()),
		is_array: Rc::new(|v| v.is_array()),
		// No JSON value is a Uint8Array.
		is_bytes: Rc::new(|_| false),
		is_object: Rc::new(is_object),
	};
}

fn leaf(pick: fn(&Leaves) -> &Check) -> Check {
	LEAVES.with(|leaves| pick(leaves).clone())
}

/// `ALWAYS`.
pub fn always() -> Check {
	leaf(|l| &l.always)
}

/// `NEVER`.
pub fn never() -> Check {
	leaf(|l| &l.never)
}

/// `IS_ARRAY`.
pub fn is_array() -> Check {
	leaf(|l| &l.is_array)
}

/// `IS_OBJECT`.
pub fn is_object_check() -> Check {
	leaf(|l| &l.is_object)
}

/// Whether `v` is a JSON object: not null, an array or bytes.
pub fn is_object(v: &Value) -> bool {
	v.is_object()
}

/// `Number.isInteger(v)`.
pub fn is_integer(v: &Value) -> bool {
	v.as_f64().is_some_and(is_integer_f64)
}

/// `Number.isInteger(x)` of a number.
pub fn is_integer_f64(x: f64) -> bool {
	x.is_finite() && x.trunc() == x
}

/// The check of a `type` name (`TYPE_CHECKS.get(name)`); `bytes` (a Uint8Array) extends JSON Schema for values that
/// never pass through JSON.
pub fn type_check_of(name: &str) -> Option<Check> {
	LEAVES.with(|l| match name {
		"string" => Some(l.is_string.clone()),
		"number" => Some(l.is_number.clone()),
		"integer" => Some(l.is_integer.clone()),
		"boolean" => Some(l.is_boolean.clone()),
		"null" => Some(l.is_null.clone()),
		"array" => Some(l.is_array.clone()),
		"object" => Some(l.is_object.clone()),
		"bytes" => Some(l.is_bytes.clone()),
		_ => None,
	})
}

/// Leaf checks that object and array loops run inline by code instead of calling them (`LEAF_CODES.get(check) ?? 0`).
pub fn leaf_code(check: &Check) -> u8 {
	LEAVES.with(|l| {
		for (leaf, code) in [
			(&l.is_string, 1),
			(&l.is_number, 2),
			(&l.is_integer, 3),
			(&l.is_boolean, 4),
			(&l.is_null, 5),
			(&l.always, 6),
		] {
			if Rc::ptr_eq(leaf, check) {
				return code;
			}
		}
		0
	})
}

fn leaf_ok(code: u8, x: &Value) -> bool {
	match code {
		1 => x.is_string(),
		2 => x.is_number(),
		3 => is_integer(x),
		4 => x.is_boolean(),
		5 => x.is_null(),
		_ => true,
	}
}

/// Whether a combined object or array check also checks the type: not at all, strictly, or passing other types.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Guard {
	/// `GUARD_NONE`.
	None,
	/// `GUARD_STRICT`.
	Strict,
	/// `GUARD_LOOSE`.
	Loose,
}

/// Flags of a declared property in an object check's entries: its leaf code and two bits.
pub const CODE: u8 = 7;
pub const REQUIRED: u8 = 8;
pub const INHERITED: u8 = 16;

/// All of `parts`; two and three parts get closures without a loop.
pub fn and(parts: Vec<Check>) -> Check {
	match parts.len() {
		0 => always(),
		1 => parts.into_iter().next().expect("one part"),
		2 => {
			let (a, b) = (parts[0].clone(), parts[1].clone());
			Rc::new(move |v| a(v) && b(v))
		}
		3 => {
			let (a, b, c) = (parts[0].clone(), parts[1].clone(), parts[2].clone());
			Rc::new(move |v| a(v) && b(v) && c(v))
		}
		_ => Rc::new(move |v| parts.iter().all(|part| part(v))),
	}
}

/// The slot of a check whose implementation is set later.
pub type Slot = Rc<RefCell<Option<Check>>>;

/// A check whose implementation is set later: the indirection a recursive reference goes through. It holds its slot
/// weakly, so a recursive schema forms no reference cycle; the compiler keeps the slot (see `Compiler::root`). A slot
/// whose schema failed to compile stays empty, and its check fails with the TypeScript's TypeError (see the file header).
pub fn indirect(slot: &Slot) -> Check {
	let slot: Weak<RefCell<Option<Check>>> = Rc::downgrade(slot);
	Rc::new(move |v| {
		let Some(check) = slot.upgrade().and_then(|slot| slot.borrow().clone()) else {
			throw(empty_slot());
			return false;
		};
		let key = Weak::as_ptr(&slot) as *const () as usize;
		reentering(key, v, || check(v)).unwrap_or(false)
	})
}

/// The TypeError of calling the empty slot of a subschema that failed to compile.
pub fn empty_slot() -> PiError {
	PiError::named("TypeError", "slot.check is not a function")
}

thread_local! {
	static ENTERED: RefCell<Vec<(usize, usize)>> = const { RefCell::new(Vec::new()) };
	static THROWN: RefCell<Option<PiError>> = const { RefCell::new(None) };
}

/// `f()` with the indirection `key` marked as checking `v`, or `None` when it already is: the check would recurse
/// without end (a schema such as `{"$ref": "#"}`), where the TypeScript runs out of stack and throws a RangeError.
/// The check fails instead of overflowing the stack, and records the RangeError (see the file header).
pub fn reentering<T>(key: usize, v: &Value, f: impl FnOnce() -> T) -> Option<T> {
	let entry = (key, v as *const Value as usize);
	if ENTERED.with(|entered| entered.borrow().contains(&entry)) {
		throw(PiError::named("RangeError", "Maximum call stack size exceeded"));
		return None;
	}
	ENTERED.with(|entered| entered.borrow_mut().push(entry));
	let result = f();
	ENTERED.with(|entered| entered.borrow_mut().pop());
	Some(result)
}

/// Records the error a running check throws in the TypeScript; the first one counts, as the throw ends the call.
pub fn throw(error: PiError) {
	THROWN.with(|thrown| {
		thrown.borrow_mut().get_or_insert(error);
	});
}

/// `f()`, or the error a check inside it threw in the TypeScript (see the file header).
pub fn catching<T>(f: impl FnOnce() -> T) -> PiResult<T> {
	let outer = THROWN.take();
	let result = f();
	match THROWN.replace(outer) {
		Some(error) => Err(error),
		None => Ok(result),
	}
}

/// `check` with `uri` pushed onto the dynamic scope while it runs (only for schemas with `$dynamicRef`).
pub fn scoped_check(scope: &Rc<RefCell<Vec<String>>>, uri: &str, check: Check) -> Check {
	let scope = scope.clone();
	let uri = uri.to_string();
	Rc::new(move |v| {
		scope.borrow_mut().push(uri.clone());
		let ok = check(v);
		scope.borrow_mut().pop();
		ok
	})
}

/// Any of the named types.
pub fn type_check(types: &[Node]) -> Check {
	let checks: Vec<Check> = types
		.iter()
		.map(|t| t.as_str().and_then(type_check_of).unwrap_or_else(never))
		.collect();
	if checks.len() == 1 {
		return checks.into_iter().next().expect("one check");
	}
	Rc::new(move |v| checks.iter().any(|check| check(v)))
}

/// A Uint8Array within the length bounds: no JSON value is one.
pub fn bytes_check(_min: f64, _max: f64) -> Check {
	Rc::new(|_| false)
}

/// The number of Unicode code points of `s`: JSON Schema string lengths count code points, not UTF-16 units. (A Rust
/// string holds no unpaired surrogate, so every `char` is one code point.)
pub fn code_points(s: &str) -> usize {
	s.chars().count()
}

fn length_ok(s: &str, min: f64, max: f64) -> bool {
	let n = code_points(s) as f64;
	n >= min && n <= max
}

/// The string keywords: length bounds, pattern and format.
pub fn string_check(strict: bool, min: f64, max: f64, re: Option<Rc<JsRegex>>, format: Option<FormatCheck>) -> Check {
	if let Some(re) = re.clone()
		&& min == 0.0
		&& max == f64::INFINITY
		&& format.is_none()
	{
		return if strict {
			Rc::new(move |v| v.as_str().is_some_and(|s| re.test(s)))
		} else {
			Rc::new(move |v| v.as_str().is_none_or(|s| re.test(s)))
		};
	}
	if re.is_none() && format.is_none() {
		return if strict {
			Rc::new(move |v| v.as_str().is_some_and(|s| length_ok(s, min, max)))
		} else {
			Rc::new(move |v| v.as_str().is_none_or(|s| length_ok(s, min, max)))
		};
	}
	Rc::new(move |v| {
		let Some(s) = v.as_str() else {
			return !strict;
		};
		length_ok(s, min, max)
			&& re.as_ref().is_none_or(|re| re.test(s))
			&& format.as_ref().is_none_or(|format| format(s))
	})
}

/// The number keywords. `mode`: 0 passes other types, 1 requires a number, 2 an integer.
pub fn number_check(mode: u8, min: f64, max: f64, xmin: f64, xmax: f64, multiple_of: Option<f64>) -> Check {
	Rc::new(move |v| {
		let Some(v) = v.as_f64() else {
			return mode == 0;
		};
		if mode == 2 && !is_integer_f64(v) {
			return false;
		}
		v >= min && v <= max && v > xmin && v < xmax && multiple_of.is_none_or(|d| is_multiple(v, d))
	})
}

/// Whether `v` is a multiple of `d`, exactly for decimals: 0.0075 / 0.0001 is 74.99999999999999 in floating point.
pub fn is_multiple(v: f64, d: f64) -> bool {
	let q = v / d;
	if is_integer_f64(q) {
		return true;
	}
	if !q.is_finite() {
		return v % d == 0.0;
	}
	let (vc, ve) = decimal(v);
	let (dc, de) = decimal(d);
	if dc == 0 {
		return false;
	}
	// The TypeScript computes these products with BigInt; modular arithmetic gives the same remainders.
	if ve >= de {
		let k = (ve - de) as u64;
		((vc % dc) * pow10_mod(k, dc)).is_multiple_of(dc)
	} else {
		let k = (de - ve) as u32;
		match 10u128.checked_pow(k).and_then(|p| p.checked_mul(dc)) {
			Some(m) => vc % m == 0,
			None => vc == 0,
		}
	}
}

// 10^k mod m.
fn pow10_mod(mut k: u64, m: u128) -> u128 {
	let mut result = 1 % m;
	let mut base = 10 % m;
	while k > 0 {
		if k & 1 == 1 {
			result = result * base % m;
		}
		base = base * base % m;
		k >>= 1;
	}
	result
}

// `Math.abs(x).toString()` split into its digits (as an integer) and the power of ten they are scaled by.
fn decimal(x: f64) -> (u128, i64) {
	let text = number_to_string(x.abs());
	let (mantissa, exponent) = match text.split_once('e') {
		Some((mantissa, exponent)) => (mantissa.to_string(), exponent.parse::<i64>().unwrap_or(0)),
		None => (text.clone(), 0),
	};
	let fraction = match mantissa.find('.') {
		Some(point) => (mantissa.len() - point - 1) as i64,
		None => 0,
	};
	let digits: u128 = mantissa.replace('.', "").parse().unwrap_or(0);
	(digits, exponent - fraction)
}

/// `inner` for objects; other values pass unless `strict`.
pub fn guard_object(inner: Check, strict: bool) -> Check {
	if strict {
		Rc::new(move |v| is_object(v) && inner(v))
	} else {
		Rc::new(move |v| !is_object(v) || inner(v))
	}
}

/// `inner` for arrays; other values pass unless `strict`.
pub fn guard_array(inner: Check, strict: bool) -> Check {
	if strict {
		Rc::new(move |v| v.is_array() && inner(v))
	} else {
		Rc::new(move |v| !v.is_array() || inner(v))
	}
}

/// One declared property of an object check: key, check, flags (leaf code, required, key also names an
/// `Object.prototype` member).
pub type Entry = (String, Check, u8);

/// An object with declared properties: `entries` has one entry per property. The value's own keys are walked, each
/// found among the declared names; `closed` rejects any other key.
pub fn declared_object(entries: Vec<Entry>, required_count: usize, closed: bool, guard: Guard) -> Check {
	Rc::new(move |v| {
		let Some(o) = v.as_object() else {
			// Guarded: by this check, or by the caller for Guard::None.
			return guard == Guard::Loose;
		};
		let mut required = 0;
		for (k, x) in o {
			let Some((_, check, f)) = entries.iter().find(|(key, _, _)| key == k) else {
				if closed {
					return false;
				}
				continue;
			};
			let c = f & CODE;
			if if c != 0 { !leaf_ok(c, x) } else { !check(x) } {
				return false;
			}
			if f & REQUIRED != 0 {
				required += 1;
			}
		}
		required == required_count
	})
}

/// An object with declared properties, required keys not declared, pattern properties and additional properties.
pub fn open_object(
	entries: Vec<Entry>,
	keys: Vec<String>,
	extra_required: Vec<String>,
	patterns: Vec<(Rc<JsRegex>, Check)>,
	additional: Option<Check>,
	guard: Guard,
) -> Check {
	let known: Option<HashSet<String>> = if !patterns.is_empty() || additional.is_some() {
		Some(keys.into_iter().collect())
	} else {
		None
	};
	Rc::new(move |v| {
		let Some(o) = v.as_object() else {
			return guard == Guard::Loose;
		};
		for (k, check, f) in &entries {
			let Some(x) = o.get(k) else {
				if f & REQUIRED != 0 {
					return false;
				}
				continue;
			};
			let c = f & CODE;
			if if c != 0 { !leaf_ok(c, x) } else { !check(x) } {
				return false;
			}
		}
		if extra_required.iter().any(|k| !o.contains_key(k)) {
			return false;
		}
		let Some(known) = &known else {
			return true;
		};
		for (k, x) in o {
			let mut matched = known.contains(k);
			for (re, check) in &patterns {
				if re.test(k) {
					matched = true;
					if !check(x) {
						return false;
					}
				}
			}
			if !matched
				&& let Some(additional) = &additional
				&& !additional(x)
			{
				return false;
			}
		}
		true
	})
}

/// Every property name passes `check` (`v` is an object).
pub fn property_names_check(check: Check) -> Check {
	Rc::new(move |v| object_of(v).keys().all(|k| check(&Value::String(k.clone()))))
}

/// `minProperties` and `maxProperties` (`v` is an object).
pub fn property_count_check(min: f64, max: f64) -> Check {
	Rc::new(move |v| {
		let n = object_of(v).len() as f64;
		n >= min && n <= max
	})
}

/// `dependentRequired` (`v` is an object).
pub fn dependent_required_check(entries: Vec<(String, Vec<String>)>) -> Check {
	Rc::new(move |v| {
		let o = object_of(v);
		for (k, list) in &entries {
			if o.contains_key(k) && list.iter().any(|r| !o.contains_key(r)) {
				return false;
			}
		}
		true
	})
}

/// `dependentSchemas` (`v` is an object).
pub fn dependent_schemas_check(entries: Vec<(String, Check)>) -> Check {
	Rc::new(move |v| {
		let o = object_of(v);
		entries.iter().all(|(k, check)| !o.contains_key(k) || check(v))
	})
}

// The object a guarded check runs on.
fn object_of(v: &Value) -> &Map<String, Value> {
	v.as_object().expect("an object check runs behind an object guard")
}

// The array a guarded check runs on.
fn array_of(v: &Value) -> &Vec<Value> {
	v.as_array().expect("an array check runs behind an array guard")
}

/// The common array: guard, length bounds, prefix items and the items after them, in one closure.
pub fn list_check(guard: Guard, min: f64, max: f64, prefix: Vec<Check>, rest: Option<Check>) -> Check {
	let n = prefix.len();
	let code = rest.as_ref().map_or(0, leaf_code);
	Rc::new(move |v| {
		let Some(list) = v.as_array() else {
			return guard == Guard::Loose;
		};
		let length = list.len();
		if (length as f64) < min || (length as f64) > max {
			return false;
		}
		let m = length.min(n);
		for i in 0..m {
			if !prefix[i](&list[i]) {
				return false;
			}
		}
		let Some(rest) = &rest else {
			return true;
		};
		if code != 0 {
			return list.iter().skip(n).all(|x| leaf_ok(code, x));
		}
		list.iter().skip(n).all(|x| rest(x))
	})
}

/// Prefix items and the items after them (`v` is an array).
pub fn items_check(prefix: Vec<Check>, rest: Option<Check>) -> Check {
	list_check(Guard::None, 0.0, f64::INFINITY, prefix, rest)
}

/// `minItems` and `maxItems` (`v` is an array).
pub fn length_check(min: f64, max: f64) -> Check {
	Rc::new(move |v| {
		let n = array_of(v).len() as f64;
		n >= min && n <= max
	})
}

/// `contains` with its bounds (`v` is an array).
pub fn contains_check(check: Check, min: f64, max: f64) -> Check {
	Rc::new(move |v| {
		let mut n = 0.0;
		for x in array_of(v) {
			if check(x) {
				n += 1.0;
				if n > max {
					return false;
				}
			}
		}
		n >= min
	})
}

/// `uniqueItems` (`v` is an array).
pub fn unique_check(v: &Value) -> bool {
	let list = array_of(v);
	for i in 1..list.len() {
		for j in 0..i {
			if deep_equal(&list[i], &list[j]) {
				return false;
			}
		}
	}
	true
}

/// `enum`: up to 8 primitives by a scan, more by a Set, objects and arrays by deep equality.
pub fn enum_check(values: &[Value]) -> Check {
	let primitives: Vec<Value> = values.iter().filter(|x| !is_object_like(x)).cloned().collect();
	if primitives.len() == values.len() && primitives.len() <= 8 {
		return Rc::new(move |v| primitives.iter().any(|p| strict_equals(p, v)));
	}
	let set: HashSet<PrimitiveKey> = primitives.iter().filter_map(PrimitiveKey::of).collect();
	let objects: Vec<Value> = values.iter().filter(|x| is_object_like(x)).cloned().collect();
	Rc::new(move |v| {
		PrimitiveKey::of(v).is_some_and(|key| set.contains(&key))
			|| (is_object_like(v) && objects.iter().any(|o| deep_equal(v, o)))
	})
}

/// `const`.
pub fn const_check(value: &Value) -> Check {
	let value = value.clone();
	if !is_object_like(&value) {
		return Rc::new(move |v| strict_equals(v, &value));
	}
	Rc::new(move |v| deep_equal(v, &value))
}

/// `anyOf`.
pub fn any_check(checks: Vec<Check>) -> Check {
	Rc::new(move |v| checks.iter().any(|check| check(v)))
}

/// `oneOf`.
pub fn one_check(checks: Vec<Check>) -> Check {
	Rc::new(move |v| {
		let mut n = 0;
		for check in &checks {
			if check(v) {
				n += 1;
				if n > 1 {
					return false;
				}
			}
		}
		n == 1
	})
}

/// `not`.
pub fn not_check(check: Check) -> Check {
	Rc::new(move |v| !check(v))
}

/// `if`, `then` and `else`.
pub fn if_check(cond: Check, then: Check, otherwise: Check) -> Check {
	Rc::new(move |v| if cond(v) { then(v) } else { otherwise(v) })
}

/// A union of objects tagged by a const on `key`: the tag selects the one branch that can match.
pub fn tagged_check(key: String, tags: Vec<Value>, checks: Vec<Check>) -> Check {
	Rc::new(move |v| {
		let Some(o) = v.as_object() else {
			return false;
		};
		let Some(tag) = o.get(&key) else {
			return false;
		};
		for (i, t) in tags.iter().enumerate() {
			if strict_equals(t, tag) {
				return checks[i](v);
			}
		}
		false
	})
}

/// JSON equality: same primitives, arrays with equal items in order, objects with the same keys and equal values.
pub fn deep_equal(a: &Value, b: &Value) -> bool {
	match (a, b) {
		(Value::Array(x), Value::Array(y)) => x.len() == y.len() && x.iter().zip(y).all(|(x, y)| deep_equal(x, y)),
		(Value::Object(x), Value::Object(y)) => {
			x.len() == y.len() && x.iter().all(|(k, xv)| y.get(k).is_some_and(|yv| deep_equal(xv, yv)))
		}
		_ => strict_equals(a, b),
	}
}

/// `a === b` for JSON values: primitives by value (numbers as `f64`), objects and arrays never (they are distinct
/// objects unless they are the same one, which a caller compares itself).
pub fn strict_equals(a: &Value, b: &Value) -> bool {
	match (a, b) {
		(Value::Null, Value::Null) => true,
		(Value::Bool(x), Value::Bool(y)) => x == y,
		(Value::Number(x), Value::Number(y)) => x.as_f64() == y.as_f64(),
		(Value::String(x), Value::String(y)) => x == y,
		_ => false,
	}
}

/// `typeof v === "object" && v !== null`.
pub fn is_object_like(v: &Value) -> bool {
	v.is_object() || v.is_array()
}

/// A primitive as a Set key (SameValueZero: `-0` and `0` are one key).
#[derive(Clone, PartialEq, Eq, Hash)]
pub enum PrimitiveKey {
	Null,
	Bool(bool),
	Number(u64),
	String(String),
}

impl PrimitiveKey {
	/// The key of a primitive, `None` for an object or array.
	pub fn of(v: &Value) -> Option<PrimitiveKey> {
		Some(match v {
			Value::Null => PrimitiveKey::Null,
			Value::Bool(b) => PrimitiveKey::Bool(*b),
			Value::Number(n) => {
				let x = n.as_f64().unwrap_or(f64::NAN);
				PrimitiveKey::Number(if x == 0.0 { 0 } else { x.to_bits() })
			}
			Value::String(s) => PrimitiveKey::String(s.clone()),
			_ => return None,
		})
	}
}

/// `String(x)` of a schema value, as `Object.hasOwn` converts a key and `Array.prototype.join` an element.
pub fn js_string(node: &Node) -> String {
	match node {
		Node::Null => "null".into(),
		Node::Bool(b) => b.to_string(),
		Node::Number(n) => number_to_string(n.as_f64().unwrap_or(f64::NAN)),
		Node::String(s) => s.to_string(),
		Node::Array(items) => js_join(items, ","),
		Node::Object(_) => "[object Object]".into(),
	}
}

/// `list.join(separator)` of schema values: `null` joins as the empty string.
pub fn js_join(items: &[Node], separator: &str) -> String {
	items
		.iter()
		.map(|item| {
			if matches!(item, Node::Null) {
				String::new()
			} else {
				js_string(item)
			}
		})
		.collect::<Vec<_>>()
		.join(separator)
}
