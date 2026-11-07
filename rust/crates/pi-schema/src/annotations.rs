//! Port of `packages/schema/src/annotations.ts`.
//! PORT-STATUS: ported
//!
//! Annotation tracking for `unevaluatedProperties` and `unevaluatedItems`: annotating checks record which
//! properties and items a schema evaluated. Only schemas that use those keywords compile into these closures.
//!
//! Accepted difference: as in `checks.rs`, a recorded throw still pops the `$dynamicRef` scope, where the TypeScript
//! RangeError skips the pop and leaves the URI on the scope for later checks.

use std::cell::RefCell;
use std::rc::{Rc, Weak};

use indexmap::IndexSet;
use serde_json::Value;

use crate::checks::{empty_slot, is_object, reentering, throw};
use crate::shared::JsRegex;
use crate::types::Check;

/// What a schema evaluated: property names (or all of them), the evaluated item prefix, items matched by `contains`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Annotations {
	pub props: Option<IndexSet<String>>,
	pub all_props: bool,
	pub items: usize,
	pub all_items: bool,
	pub indexes: Option<IndexSet<usize>>,
}

/// A check that records what it evaluated into `a` when it passes.
pub type AnnotatingCheck = Rc<dyn Fn(&Value, &mut Annotations) -> bool>;

/// Empty annotations.
pub fn new_annotations() -> Annotations {
	Annotations::default()
}

fn merge(a: &mut Annotations, b: &Annotations) {
	if let Some(props) = &b.props {
		for k in props {
			add_prop(a, k);
		}
	}
	a.all_props = a.all_props || b.all_props;
	if b.items > a.items {
		a.items = b.items;
	}
	a.all_items = a.all_items || b.all_items;
	if let Some(indexes) = &b.indexes {
		for &i in indexes {
			add_index(a, i);
		}
	}
}

fn add_prop(a: &mut Annotations, key: &str) {
	a.props.get_or_insert_with(IndexSet::new).insert(key.to_string());
}

fn add_index(a: &mut Annotations, index: usize) {
	a.indexes.get_or_insert_with(IndexSet::new).insert(index);
}

/// `ANNOTATING_TRUE`.
pub fn annotating_true() -> AnnotatingCheck {
	ANNOTATING.with(|(yes, _)| yes.clone())
}

/// `ANNOTATING_FALSE`.
pub fn annotating_false() -> AnnotatingCheck {
	ANNOTATING.with(|(_, no)| no.clone())
}

thread_local! {
	static ANNOTATING: (AnnotatingCheck, AnnotatingCheck) = (Rc::new(|_, _| true), Rc::new(|_, _| false));
}

/// The slot of an annotating check whose implementation is set later.
pub type AnnotatingSlot = Rc<RefCell<Option<AnnotatingCheck>>>;

/// The indirection a recursive reference goes through (holding its slot weakly, as `checks::indirect` does).
pub fn indirect_annotating(slot: &AnnotatingSlot) -> AnnotatingCheck {
	let slot: Weak<RefCell<Option<AnnotatingCheck>>> = Rc::downgrade(slot);
	Rc::new(move |v, a| {
		let Some(check) = slot.upgrade().and_then(|slot| slot.borrow().clone()) else {
			throw(empty_slot());
			return false;
		};
		let key = Weak::as_ptr(&slot) as *const () as usize;
		reentering(key, v, || check(v, a)).unwrap_or(false)
	})
}

/// A plain check in annotating position: it evaluates nothing that unevaluated* sees.
pub fn ignore_annotations(check: Check) -> AnnotatingCheck {
	Rc::new(move |v, _| check(v))
}

/// An annotating check run as a plain check, with fresh annotations.
pub fn with_fresh_annotations(check: AnnotatingCheck) -> Check {
	Rc::new(move |v| check(v, &mut new_annotations()))
}

/// `check` with `uri` pushed onto the dynamic scope while it runs.
pub fn scoped_annotating(scope: &Rc<RefCell<Vec<String>>>, uri: &str, check: AnnotatingCheck) -> AnnotatingCheck {
	let scope = scope.clone();
	let uri = uri.to_string();
	Rc::new(move |v, a| {
		scope.borrow_mut().push(uri.clone());
		let ok = check(v, a);
		scope.borrow_mut().pop();
		ok
	})
}

/// unevaluated* sees only the annotations of its own schema, never those of its siblings in the parent.
pub fn isolate_annotations(check: AnnotatingCheck) -> AnnotatingCheck {
	Rc::new(move |v, a| {
		let mut b = new_annotations();
		if !check(v, &mut b) {
			return false;
		}
		merge(a, &b);
		true
	})
}

/// The non-annotating keywords, then each annotating part in order.
pub fn annotate_all(plain: Check, parts: Vec<AnnotatingCheck>) -> AnnotatingCheck {
	Rc::new(move |v, a| {
		if !plain(v) {
			return false;
		}
		parts.iter().all(|part| part(v, a))
	})
}

/// `properties`, `patternProperties` and `additionalProperties`.
pub fn annotate_object(
	props: Vec<(String, Check)>,
	patterns: Vec<(Rc<JsRegex>, Check)>,
	additional: Option<Check>,
) -> AnnotatingCheck {
	let declared: IndexSet<String> = props.iter().map(|(k, _)| k.clone()).collect();
	Rc::new(move |v, a| {
		if !is_object(v) {
			return true;
		}
		let o = v.as_object().expect("checked by is_object");
		let mut seen: Vec<&String> = Vec::new();
		for (k, check) in &props {
			let Some(x) = o.get(k) else {
				continue;
			};
			if !check(x) {
				return false;
			}
			seen.push(k);
		}
		for (k, x) in o {
			let mut matched = declared.contains(k);
			for (re, check) in &patterns {
				if re.test(k) {
					if !check(x) {
						return false;
					}
					matched = true;
					seen.push(k);
				}
			}
			if !matched && let Some(additional) = &additional {
				if !additional(x) {
					return false;
				}
				seen.push(k);
			}
		}
		for k in seen {
			add_prop(a, k);
		}
		true
	})
}

/// `prefixItems`/`items`/`additionalItems` and `contains` with its bounds.
pub fn annotate_array(
	prefix: Vec<Check>,
	rest: Option<Check>,
	contains: Option<Check>,
	min_contains: f64,
	max_contains: f64,
) -> AnnotatingCheck {
	Rc::new(move |v, a| {
		let Some(list) = v.as_array() else {
			return true;
		};
		let m = list.len().min(prefix.len());
		for i in 0..m {
			if !prefix[i](&list[i]) {
				return false;
			}
		}
		if let Some(rest) = &rest {
			for x in list.iter().skip(prefix.len()) {
				if !rest(x) {
					return false;
				}
			}
			a.all_items = true;
		}
		if m > a.items {
			a.items = m;
		}
		if let Some(contains) = &contains {
			let mut n = 0.0;
			for (i, x) in list.iter().enumerate() {
				if contains(x) {
					n += 1.0;
					add_index(a, i);
				}
			}
			if n < min_contains || n > max_contains {
				return false;
			}
		}
		true
	})
}

/// `anyOf` (or `oneOf` when `exactly_one`): every passing branch contributes its annotations.
pub fn annotate_any(branches: Vec<AnnotatingCheck>, exactly_one: bool) -> AnnotatingCheck {
	Rc::new(move |v, a| {
		let mut n = 0;
		for branch in &branches {
			let mut b = new_annotations();
			if branch(v, &mut b) {
				n += 1;
				merge(a, &b);
			}
		}
		if exactly_one { n == 1 } else { n > 0 }
	})
}

/// `if`, `then` and `else`: a passing `if` contributes its annotations even without `then`.
pub fn annotate_if(
	cond: AnnotatingCheck,
	then: Option<AnnotatingCheck>,
	otherwise: Option<AnnotatingCheck>,
) -> AnnotatingCheck {
	Rc::new(move |v, a| {
		let mut b = new_annotations();
		if cond(v, &mut b) {
			merge(a, &b);
			return then.as_ref().is_none_or(|then| then(v, a));
		}
		otherwise.as_ref().is_none_or(|otherwise| otherwise(v, a))
	})
}

/// One `dependentSchemas` entry.
pub fn annotate_dependent(key: String, check: AnnotatingCheck) -> AnnotatingCheck {
	Rc::new(move |v, a| !v.as_object().is_some_and(|o| o.contains_key(&key)) || check(v, a))
}

/// `unevaluatedProperties`: every property no other keyword evaluated passes `check`.
pub fn unevaluated_properties(check: Check) -> AnnotatingCheck {
	Rc::new(move |v, a| {
		let Some(o) = v.as_object() else {
			return true;
		};
		if a.all_props {
			return true;
		}
		for (k, x) in o {
			if !a.props.as_ref().is_some_and(|props| props.contains(k)) && !check(x) {
				return false;
			}
		}
		a.all_props = true;
		true
	})
}

/// `unevaluatedItems`: every item no other keyword evaluated passes `check`.
pub fn unevaluated_items(check: Check) -> AnnotatingCheck {
	Rc::new(move |v, a| {
		let Some(list) = v.as_array() else {
			return true;
		};
		if a.all_items {
			return true;
		}
		for (i, item) in list.iter().enumerate().skip(a.items) {
			if !a.indexes.as_ref().is_some_and(|indexes| indexes.contains(&i)) && !check(item) {
				return false;
			}
		}
		a.all_items = true;
		true
	})
}
