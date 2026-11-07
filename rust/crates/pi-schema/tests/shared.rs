//! Port of `packages/schema/test/shared.test.ts`: values shared across compiled schemas hold nothing alive: a cached
//! pattern goes once its schemas are dropped.
//!
//! Rust port: reference counting drops values at once, so the TypeScript's garbage collection rounds are not needed, and
//! a `Weak` of the check stands in for the `WeakRef`.

use std::rc::Rc;

use pi_schema::shared::shared_regex_count;
use pi_schema::{CompileOptions, Validator, compile};
use serde_json::json;

fn plain(schema: serde_json::Value) -> Validator {
	compile(&schema, &CompileOptions::default()).unwrap()
}

mod shared_checks {
	use super::*;

	#[test]
	fn share_the_module_level_check_of_a_bare_type() {
		let a = plain(json!({ "type": "string", "description": "a" }));
		let b = plain(json!({ "type": "string", "title": "b" }));
		assert!(Rc::ptr_eq(&a.check, &b.check));
	}

	#[test]
	fn share_a_pattern_s_regular_expression_and_drop_it_with_the_last_schema_that_uses_it() {
		let before = shared_regex_count();
		let validators: Vec<Validator> = (0..50)
			.map(|i| plain(json!({ "type": "string", "pattern": format!("^unique-{i}$") })))
			.collect();
		let again = plain(json!({ "type": "string", "pattern": "^unique-0$" }));
		assert_eq!(shared_regex_count(), before + 50);
		assert!(again.check(&json!("unique-0")));
		let check = Rc::downgrade(&validators[0].check);
		drop(validators);
		// `again` still uses ^unique-0$; the other 49 patterns are gone.
		assert!(check.upgrade().is_none());
		assert_eq!(shared_regex_count(), before + 1);
	}
}
