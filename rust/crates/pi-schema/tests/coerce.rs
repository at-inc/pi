//! Port of `packages/schema/test/coerce.test.ts`: compile_coercing(): coerce() and parse(), the three coercion passes
//! (optional nulls, TypeBox's conversion rules, primitive coercion by type), their order where they conflict, and the
//! guarantees around them. Ports pi-ai's tool argument coercion tests.
//!
//! Rust port: `toBe(value)` identity of the whole value is `Cow::Borrowed`; identity of an unchanged member cannot be
//! observed and is checked by equality. The TypeBox schemas (`Type.Cyclic`, `Type.Record`) are the JSON TypeBox 1.3.27
//! builds. The TypeScript's static-type assertions and its `Function`-constructor test (Rust compiles no code from
//! strings) have no runtime counterpart.

use std::borrow::Cow;

use pi_schema::formats::formats;
use pi_schema::{
	CoercingValidator, CompileOptions, ParseResult, ValidationError, arr, compile, compile_coercing, int, map,
	nullable, num, obj, opt, str, tuple,
};
use serde_json::{Value, json};

fn coercing(schema: &Value) -> CoercingValidator {
	compile_coercing(schema, &CompileOptions::default()).unwrap()
}

fn coerce(schema: &Value, value: &Value) -> Value {
	let options = CompileOptions {
		formats: Some(formats()),
		..CompileOptions::default()
	};
	compile_coercing(schema, &options)
		.unwrap()
		.coerce(value)
		.unwrap()
		.into_owned()
}

/// The coerced `value` property of `{ value }` under an object schema with one required property `value`.
fn coerce_value(schema: Value, value: Value) -> Value {
	let root = json!({ "type": "object", "properties": { "value": schema }, "required": ["value"] });
	coerce(&root, &json!({ "value": value }))["value"].clone()
}

fn parse(validator: &CoercingValidator, value: Value) -> ParseResult {
	validator.parse(&value).unwrap()
}

fn error(path: &str, keyword: &str, message: &str) -> ValidationError {
	ValidationError {
		path: path.into(),
		keyword: keyword.into(),
		message: message.into(),
	}
}

fn none() -> Value {
	Value::Null
}

mod compile_options {
	use super::*;

	#[test]
	fn adds_coerce_and_parse_only_with_coerce_true() {
		// The plain Validator has no coerce() or parse() (a compile error to call); compile_coercing() adds them.
		let plain = compile(&obj([("n", num(none()).into())], none()), &CompileOptions::default()).unwrap();
		assert!(plain.check(&json!({ "n": 1 })));
		let coercing = coercing(&obj([("n", num(none()).into())], none()));
		assert!(coercing.check(&json!({ "n": 1 })));
		assert!(parse(&coercing, json!({ "n": "1" })).ok());
	}
}

mod coerce {
	use super::*;

	#[test]
	fn never_modifies_the_value() {
		let value = json!({ "count": "42", "tags": "a" });
		let schema = obj(
			[("count", num(none()).into()), ("tags", arr(str(none()), none()).into())],
			none(),
		);
		assert_eq!(coerce(&schema, &value), json!({ "count": 42, "tags": ["a"] }));
		assert_eq!(value, json!({ "count": "42", "tags": "a" }));
	}

	#[test]
	fn returns_the_value_itself_when_nothing_changes() {
		let value = json!({ "a": { "b": [1, 2] }, "c": "x" });
		let schema = obj(
			[
				("a", obj([("b", arr(num(none()), none()).into())], none()).into()),
				("c", str(none()).into()),
			],
			none(),
		);
		let validator = coercing(&schema);
		assert!(matches!(validator.coerce(&value).unwrap(), Cow::Borrowed(v) if std::ptr::eq(v, &value)));
	}

	#[test]
	fn copies_only_the_changed_paths() {
		let unchanged = json!({ "b": [1, 2] });
		let value = json!({ "a": unchanged, "c": { "d": "1" } });
		let schema = obj(
			[
				("a", obj([("b", arr(num(none()), none()).into())], none()).into()),
				("c", obj([("d", num(none()).into())], none()).into()),
			],
			none(),
		);
		let validator = coercing(&schema);
		let coerced = validator.coerce(&value).unwrap();
		assert!(matches!(coerced, Cow::Owned(_)));
		assert_eq!(*coerced, json!({ "a": { "b": [1, 2] }, "c": { "d": 1 } }));
		assert_eq!(coerced["a"], unchanged);
		assert_eq!(value["c"], json!({ "d": "1" }));
	}

	#[test]
	fn works_on_frozen_schemas() {
		// Rust values are immutable through a shared reference, frozen or not.
		let schema = json!({ "type": "object", "properties": { "n": { "type": "integer" } } });
		assert_eq!(coerce(&schema, &json!({ "n": "7" })), json!({ "n": 7 }));
	}

	#[test]
	fn leaves_values_it_cannot_coerce_for_the_check_to_report() {
		let validator = coercing(&obj([("count", num(none()).into())], none()));
		let input = json!({ "count": "many" });
		let coerced = validator.coerce(&input).unwrap();
		assert_eq!(*coerced, json!({ "count": "many" }));
		assert!(!validator.check(&coerced));
	}
}

mod parse {
	use super::*;

	#[test]
	fn returns_the_coerced_value_when_it_is_valid() {
		let validator = coercing(&obj(
			[("count", num(none()).into()), ("tags", arr(str(none()), none()).into())],
			none(),
		));
		assert_eq!(
			parse(&validator, json!({ "count": "42", "tags": "a" })),
			ParseResult::Ok(json!({ "count": 42, "tags": ["a"] }))
		);
	}

	#[test]
	fn returns_the_errors_of_the_coerced_value_when_it_is_invalid() {
		let validator = coercing(&obj(
			[("count", num(none()).into()), ("name", str(none()).into())],
			none(),
		));
		assert_eq!(
			parse(&validator, json!({ "count": "many", "name": 7 })),
			ParseResult::Err(vec![error("/count", "type", "expected number, got string")])
		);
	}
}

mod pass_1_optional_nulls {
	use super::*;

	#[test]
	fn drops_null_in_optional_non_nullable_properties_also_nested() {
		let schema = obj(
			[
				("path", str(none()).into()),
				("offset", opt(num(none())).into()),
				("nullable", opt(nullable(str(none()), none())).into()),
				(
					"metadata",
					obj([("enabled", opt(json!({ "type": "boolean" })).into())], none()).into(),
				),
			],
			json!({ "open": true }),
		);
		assert_eq!(
			coerce(
				&schema,
				&json!({ "path": "file.txt", "offset": null, "nullable": null, "metadata": { "enabled": null } })
			),
			json!({ "path": "file.txt", "nullable": null, "metadata": {} })
		);
	}

	#[test]
	fn keeps_a_null_whose_property_is_a_ref_the_reference_may_allow_it() {
		let schema = json!({
			"type": "object",
			"properties": { "value": { "$ref": "#/$defs/value" } },
			"$defs": { "value": { "anyOf": [{ "type": "number" }, { "type": "null" }] } },
		});
		assert_eq!(coerce(&schema, &json!({ "value": null })), json!({ "value": null }));
	}

	#[test]
	fn walks_array_items() {
		let schema = json!({ "type": "array", "items": obj([("a", str(none()).into()), ("b", opt(int(none())).into())], none()) });
		assert_eq!(
			coerce(&schema, &json!([{ "a": "x", "b": null }])),
			json!([{ "a": "x" }])
		);
	}
}

mod pass_2_type_box_s_conversion_rules_on_json_schema {
	use super::*;

	#[test]
	fn converts_primitives_by_type() {
		assert_eq!(coerce_value(json!({ "type": "number" }), json!("42")), json!(42));
		assert_eq!(coerce_value(json!({ "type": "number" }), json!("")), json!(0));
		assert_eq!(coerce_value(json!({ "type": "number" }), json!("true")), json!(1));
		assert_eq!(coerce_value(json!({ "type": "integer" }), json!("3.5")), json!(3));
		assert_eq!(coerce_value(json!({ "type": "integer" }), json!(1.5)), json!(1));
		assert_eq!(coerce_value(json!({ "type": "boolean" }), json!("1")), json!(true));
		assert_eq!(coerce_value(json!({ "type": "boolean" }), json!("FALSE")), json!(false));
		assert_eq!(coerce_value(json!({ "type": "null" }), json!("null")), json!(null));
		assert_eq!(coerce_value(json!({ "type": "string" }), json!(42)), json!("42"));
	}

	#[test]
	fn wraps_a_single_value_into_an_array_and_converts_the_items() {
		assert_eq!(
			coerce_value(json!({ "type": "array", "items": { "type": "number" } }), json!("1")),
			json!([1])
		);
		assert_eq!(
			coerce_value(json!({ "type": "array", "items": { "type": "string" } }), json!(null)),
			json!(["null"])
		);
	}

	#[test]
	fn converts_tuple_items_in_place_and_never_wraps_for_a_tuple() {
		let tuple =
			json!({ "type": "array", "prefixItems": [{ "type": "number" }, { "type": "boolean" }], "items": false });
		assert_eq!(coerce_value(tuple.clone(), json!(["1", "true"])), json!([1, true]));
		assert_eq!(coerce_value(tuple, json!("1")), json!("1"));
	}

	#[test]
	fn converts_const_and_enum_values_only_when_the_result_is_a_member() {
		assert_eq!(coerce_value(json!({ "const": 1 }), json!("1")), json!(1));
		assert_eq!(coerce_value(json!({ "const": 1 }), json!("2")), json!("2"));
		assert_eq!(coerce_value(json!({ "enum": [1, 2, "hello"] }), json!("2")), json!(2));
		assert_eq!(
			coerce_value(json!({ "enum": [1, 2, "hello"] }), json!("world")),
			json!("world")
		);
	}

	#[test]
	fn keeps_a_value_one_union_branch_accepts_else_takes_the_first_branch_whose_conversion_the_union_accepts() {
		let union = json!({ "anyOf": [{ "type": "number" }, { "type": "null" }] });
		assert_eq!(coerce_value(union.clone(), json!(null)), json!(null));
		assert_eq!(coerce_value(union.clone(), json!("42")), json!(42));
		assert_eq!(coerce_value(union, json!("null")), json!(null));
		assert_eq!(
			coerce_value(
				json!({ "oneOf": [{ "type": "boolean" }, { "type": "number" }] }),
				json!("1")
			),
			json!(true)
		);
	}

	#[test]
	fn converts_all_of_with_each_member_in_turn() {
		let schema = json!({
			"allOf": [
				obj([("a", num(none()).into())], json!({ "open": true })),
				obj([("b", json!({ "type": "boolean" }).into())], json!({ "open": true })),
			],
		});
		assert_eq!(
			coerce(&schema, &json!({ "a": "1", "b": "true" })),
			json!({ "a": 1, "b": true })
		);
	}

	#[test]
	fn follows_ref() {
		let schema = json!({
			"type": "object",
			"properties": { "item": { "$ref": "#/$defs/item" } },
			"$defs": { "item": { "type": "object", "properties": { "id": { "type": "integer" } } } },
		});
		assert_eq!(
			coerce(&schema, &json!({ "item": { "id": "7" } })),
			json!({ "item": { "id": 7 } })
		);
	}

	#[test]
	fn converts_pattern_properties_and_additional_properties() {
		let schema = json!({
			"type": "object",
			"properties": { "a": { "type": "number" } },
			"patternProperties": { "^n_": { "type": "number" } },
			"additionalProperties": { "type": "boolean" },
		});
		assert_eq!(
			coerce(&schema, &json!({ "a": "1", "n_x": "2", "other": "true" })),
			json!({ "a": 1, "n_x": 2, "other": true })
		);
	}
}

mod pass_3_primitive_coercion_by_type {
	use super::*;

	#[test]
	fn coerces_like_ajv_s_coerce_types() {
		let cases = [
			(json!({ "type": "number" }), json!(true), json!(1)),
			(json!({ "type": "number" }), json!(null), json!(0)),
			(json!({ "type": "integer" }), json!("42"), json!(42)),
			(json!({ "type": "boolean" }), json!("true"), json!(true)),
			(json!({ "type": "boolean" }), json!(0), json!(false)),
			(json!({ "type": "string" }), json!(true), json!("true")),
			(json!({ "type": "null" }), json!(""), json!(null)),
			(json!({ "type": "null" }), json!(0), json!(null)),
			(json!({ "type": "null" }), json!(false), json!(null)),
		];
		for (schema, input, expected) in cases {
			assert_eq!(coerce_value(schema, input), expected);
		}
	}

	#[test]
	fn leaves_a_value_that_matches_one_of_a_type_array_s_types_else_takes_the_first_coercible_type() {
		assert_eq!(
			coerce_value(json!({ "type": ["number", "string"] }), json!("1")),
			json!("1")
		);
		assert_eq!(
			coerce_value(json!({ "type": ["boolean", "number"] }), json!("1")),
			json!(1)
		);
		assert_eq!(
			coerce_value(json!({ "type": ["integer", "null"] }), json!("5")),
			json!(5)
		);
	}

	#[test]
	fn accepts_null_for_nullable_array_schemas_with_items() {
		assert_eq!(
			coerce_value(
				json!({ "type": ["array", "null"], "items": { "type": "string" } }),
				json!(null)
			),
			json!(null)
		);
	}

	#[test]
	fn coerces_nullable_unions_when_the_value_matches_no_branch() {
		assert_eq!(
			coerce_value(
				json!({ "anyOf": [{ "type": "number" }, { "type": "null" }] }),
				json!("42")
			),
			json!(42)
		);
		assert_eq!(
			coerce_value(
				json!({ "oneOf": [{ "type": "number" }, { "type": "null" }] }),
				json!(null)
			),
			json!(null)
		);
	}

	#[test]
	fn leaves_values_no_rule_coerces() {
		let validator = coercing(
			&json!({ "type": "object", "properties": { "value": { "type": "integer" } }, "required": ["value"] }),
		);
		for input in [json!("abc"), json!("4x"), json!({}), json!(["1", "2"])] {
			let value = json!({ "value": input });
			let coerced = validator.coerce(&value).unwrap();
			assert!(!validator.check(&coerced));
		}
	}
}

mod where_the_passes_conflict_the_conversion_rules_win {
	use super::*;

	#[test]
	fn turns_null_into_null_for_a_required_string_pass_3_alone_would_give_empty() {
		assert_eq!(coerce_value(json!({ "type": "string" }), json!(null)), json!("null"));
	}

	#[test]
	fn turns_empty_into_0_for_a_number_pass_3_leaves_an_empty_string() {
		assert_eq!(coerce_value(json!({ "type": "number" }), json!("")), json!(0));
	}

	#[test]
	fn turns_1_and_0_into_booleans_pass_3_takes_only_true_false_1_and_0() {
		assert_eq!(coerce_value(json!({ "type": "boolean" }), json!("0")), json!(false));
	}

	#[test]
	fn truncates_42_1_for_an_integer_pass_3_rejects_it() {
		assert_eq!(coerce_value(json!({ "type": "integer" }), json!("42.1")), json!(42));
	}

	#[test]
	fn drops_an_optional_null_before_converting_it_pass_2_alone_would_give_null() {
		assert_eq!(
			coerce(
				&obj([("name", opt(str(none())).into())], none()),
				&json!({ "name": null })
			),
			json!({})
		);
	}
}

mod recursive_schemas {
	use super::*;

	fn tree() -> Value {
		json!({
			"type": "object",
			"properties": { "t": { "$ref": "#/$defs/Tree" } },
			"$defs": { "Tree": { "anyOf": [{ "type": "string" }, { "type": "array", "items": { "$ref": "#/$defs/Tree" } }] } },
		})
	}

	fn json_schema() -> Value {
		json!({
			"type": "object",
			"properties": { "data": { "$ref": "#/$defs/Json" } },
			"required": ["data"],
			"$defs": {
				"Json": {
					"anyOf": [
						{ "type": "string" },
						{ "type": "number" },
						{ "type": "boolean" },
						{ "type": "array", "items": { "$ref": "#/$defs/Json" } },
						{ "type": "object", "additionalProperties": { "$ref": "#/$defs/Json" } },
					],
				},
			},
		})
	}

	#[test]
	fn convert_through_a_recursive_union_by_its_first_accepting_branch() {
		let validator = coercing(&tree());
		assert_eq!(
			parse(&validator, json!({ "t": ["a", ["b"]] })),
			ParseResult::Ok(json!({ "t": ["a", ["b"]] }))
		);
		assert_eq!(
			parse(&validator, json!({ "t": 5 })),
			ParseResult::Ok(json!({ "t": "5" }))
		);
		assert_eq!(
			parse(&validator, json!({ "t": [5] })),
			ParseResult::Ok(json!({ "t": ["5"] }))
		);
		assert_eq!(
			parse(&coercing(&json_schema()), json!({ "data": { "a": [1, null] } })),
			ParseResult::Ok(json!({ "data": { "a": [1, "null"] } }))
		);
	}

	#[test]
	fn leave_a_value_a_recursive_schema_cannot_convert_as_it_is_for_the_check_to_report() {
		let validator = coercing(&tree());
		assert!(!parse(&validator, json!({ "t": {} })).ok());
		assert_eq!(
			*validator.coerce(&json!({ "t": { "a": 1 } })).unwrap(),
			json!({ "t": { "a": 1 } })
		);
	}

	#[test]
	fn convert_a_scalar_or_array_schema_that_refers_to_itself() {
		let schema = json!({
			"type": "object",
			"properties": {
				"value": { "anyOf": [{ "type": "number" }, { "type": "array", "items": { "$ref": "#/properties/value" } }] },
			},
			"required": ["value"],
		});
		let validator = coercing(&schema);
		assert_eq!(
			parse(&validator, json!({ "value": "42" })),
			ParseResult::Ok(json!({ "value": 42 }))
		);
		assert_eq!(
			parse(&validator, json!({ "value": ["1", ["2"]] })),
			ParseResult::Ok(json!({ "value": [1, [2]] }))
		);
		assert!(!parse(&validator, json!({ "value": {} })).ok());
	}

	#[test]
	#[ignore = "a JSON value cannot cycle through shared objects; the $ref form is covered by the next test"]
	fn stop_at_a_schema_that_cycles_through_shared_js_objects_instead_of_ref() {}

	#[test]
	fn reset_the_re_entry_state_between_calls_and_leave_the_input_unchanged() {
		let validator =
			coercing(&json!({ "anyOf": [{ "type": "number" }, { "type": "array", "items": { "$ref": "#" } }] }));
		for _ in 0..20 {
			assert!(!parse(&validator, json!({})).ok());
			assert_eq!(parse(&validator, json!("42")), ParseResult::Ok(json!(42)));
			let input = json!([1, "2", ["3"]]);
			assert_eq!(validator.parse(&input).unwrap(), ParseResult::Ok(json!([1, 2, [3]])));
			assert_eq!(input, json!([1, "2", ["3"]]));
		}
	}

	#[test]
	fn convert_and_explain_a_deep_value_that_every_branch_of_a_recursive_union_rejects_in_linear_time() {
		// Each object branch converts and explains every child: without memoizing per call this is 2^depth work.
		let s = json!({
			"anyOf": [
				{ "type": "object", "additionalProperties": { "$ref": "#/$defs/S" }, "required": ["a"] },
				{ "type": "object", "additionalProperties": { "$ref": "#/$defs/S" }, "required": ["b"] },
			],
		});
		let validator =
			coercing(&json!({ "type": "object", "properties": { "s": { "$ref": "#/$defs/S" } }, "$defs": { "S": s } }));
		let mut value = json!({ "x": 1 });
		for _ in 0..40 {
			value = json!({ "c": value });
		}
		let result = parse(&validator, json!({ "s": value }));
		assert!(!result.ok());
		let mut errors: Vec<ValidationError> = (0..41)
			.map(|i| error(&format!("/s{}/a", "/c".repeat(i)), "required", "missing"))
			.collect();
		errors.push(error(
			&format!("/s{}/x", "/c".repeat(40)),
			"type",
			"expected object, got integer",
		));
		assert_eq!(result, ParseResult::Err(errors));
	}

	#[test]
	fn convert_a_type_box_cyclic_union() {
		// Type.Object({ t: Type.Cyclic({ Tree: Type.Union([Type.String(), Type.Array(Type.Ref("Tree"))]) }, "Tree") })
		let schema = json!({"type":"object","required":["t"],"properties":{"t":{"$defs":{"Tree":{"anyOf":[{"type":"string"},{"type":"array","items":{"$ref":"Tree"}}],"$id":"Tree"}},"$ref":"Tree"}}});
		let validator = coercing(&schema);
		assert_eq!(
			parse(&validator, json!({ "t": [5] })),
			ParseResult::Ok(json!({ "t": ["5"] }))
		);
		assert!(!parse(&validator, json!({ "t": { "a": 1 } })).ok());
	}
}

mod member_keywords_across_the_passes {
	use super::*;

	#[test]
	fn keep_a_pattern_matched_property_from_additional_properties_in_every_pass() {
		// Type.Record(Type.String(), Type.Number(), { additionalProperties: Type.String() })
		let record = json!({"type":"object","patternProperties":{"^.*$":{"type":"number"}},"additionalProperties":{"type":"string"}});
		assert_eq!(
			parse(&coercing(&record), json!({ "n1": "1" })),
			ParseResult::Ok(json!({ "n1": 1 }))
		);
		let plain = json!({
			"type": "object",
			"patternProperties": { "^n_": { "type": ["integer", "null"] } },
			"additionalProperties": { "type": "string" },
		});
		assert_eq!(
			coerce(&plain, &json!({ "n_a": "5", "other": 7 })),
			json!({ "n_a": 5, "other": "7" })
		);
	}

	#[test]
	fn drop_optional_nulls_inside_declared_pattern_and_additional_property_values_alike() {
		let item = obj([("n", opt(int(json!({ "minimum": 1 }))).into())], none());
		let schemas = [
			obj([("entry", item.clone().into())], none()),
			map(item.clone(), none()),
			json!({ "type": "object", "patternProperties": { "^entry$": item }, "additionalProperties": false }),
		];
		for schema in schemas {
			let input = json!({ "entry": { "n": null } });
			assert_eq!(
				coercing(&schema).parse(&input).unwrap(),
				ParseResult::Ok(json!({ "entry": {} }))
			);
			assert_eq!(input, json!({ "entry": { "n": null } }));
		}
	}

	#[test]
	fn keep_a_null_under_pattern_and_additional_properties_their_keys_are_not_optional_properties() {
		let schema = json!({ "type": "object", "additionalProperties": { "type": ["integer", "null"] } });
		assert_eq!(
			parse(&coercing(&schema), json!({ "n": null })),
			ParseResult::Ok(json!({ "n": null }))
		);
	}

	#[test]
	fn drop_optional_nulls_in_tuple_items_as_in_array_items() {
		let item = obj([("n", opt(int(none())).into())], none());
		assert_eq!(
			*coercing(&arr(item.clone(), none()))
				.coerce(&json!([{ "n": null }]))
				.unwrap(),
			json!([{}])
		);
		assert_eq!(
			*coercing(&tuple(vec![item], none()))
				.coerce(&json!([{ "n": null }]))
				.unwrap(),
			json!([{}])
		);
	}

	#[test]
	fn coerce_tuple_items_by_type_in_pass_3() {
		let schema = json!({ "type": "array", "prefixItems": [{ "type": ["integer", "null"] }, { "type": ["boolean", "null"] }] });
		assert_eq!(coerce(&schema, &json!(["5", "true"])), json!([5, true]));
	}

	#[test]
	fn convert_the_properties_a_union_node_declares_besides_its_branches() {
		let schema = json!({
			"type": "object",
			"properties": { "a": { "type": "integer" }, "b": { "type": "integer" } },
			"anyOf": [{ "required": ["a"] }, { "required": ["b"] }],
		});
		assert_eq!(
			parse(&coercing(&schema), json!({ "a": "3.5" })),
			ParseResult::Ok(json!({ "a": 3 }))
		);
	}

	fn candidates_after_own_properties(keyword: &str) {
		let mut schema = json!({
			"type": "object",
			"properties": { "c": { "type": "integer" } },
			"required": ["c"],
		});
		// The TypeScript literal puts the union keyword after "type": rebuild the key order.
		let branches = json!([
			{ "type": "object", "properties": { "a": { "type": "array", "items": { "type": "integer" } } }, "required": ["a"] },
			{ "type": "object", "properties": { "b": { "type": "boolean" } }, "required": ["b"] },
		]);
		let object = schema.as_object_mut().unwrap();
		let rest: Vec<(String, Value)> = std::mem::take(object).into_iter().collect();
		object.insert("type".into(), json!("object"));
		object.insert(keyword.into(), branches);
		for (k, v) in rest.into_iter().filter(|(k, _)| k != "type") {
			object.insert(k, v);
		}
		let input = json!({ "a": "3", "c": "2" });
		assert_eq!(
			coercing(&schema).parse(&input).unwrap(),
			ParseResult::Ok(json!({ "a": [3], "c": 2 }))
		);
		assert_eq!(input, json!({ "a": "3", "c": "2" }));
	}

	#[test]
	fn check_any_of_candidates_after_the_node_s_own_properties_convert() {
		candidates_after_own_properties("anyOf");
	}

	#[test]
	fn check_one_of_candidates_after_the_node_s_own_properties_convert() {
		candidates_after_own_properties("oneOf");
	}
}
