//! Port of `packages/schema/test/validator.test.ts`: compile() and Validator: checks, errors, values, options and
//! schemas from other sources.
//!
//! Rust port: JSON has no `undefined` and no `Uint8Array`, and Rust strings hold no lone surrogates. The cases built on
//! them check what remains observable (an absent property; `bytes` rejecting every JSON value; surrogate pairs) and say
//! so. The TypeBox schemas are the JSON TypeBox 1.3.27 builds.

use pi_schema::formats::formats;
use pi_schema::{
	CompileOptions, Draft, ParseResult, UnresolvedRefs, ValidationError, Validator, compile, compile_coercing,
};
use serde_json::{Value, json};

fn read() -> Value {
	json!({
		"type": "object",
		"properties": { "path": { "type": "string" }, "offset": { "type": "integer", "minimum": 1 } },
		"required": ["path"],
		"additionalProperties": false,
	})
}

fn block() -> Value {
	json!({
		"anyOf": [
			{
				"type": "object",
				"properties": { "kind": { "const": "text" }, "text": { "type": "string" } },
				"required": ["kind", "text"],
				"additionalProperties": false,
			},
			{
				"type": "object",
				"properties": { "kind": { "const": "image" }, "width": { "type": "integer", "minimum": 0 } },
				"required": ["kind", "width"],
				"additionalProperties": false,
			},
		],
	})
}

fn plain(schema: &Value) -> Validator {
	compile(schema, &CompileOptions::default()).unwrap()
}

fn with_formats() -> CompileOptions {
	CompileOptions {
		formats: Some(formats()),
		..CompileOptions::default()
	}
}

fn error(path: &str, keyword: &str, message: &str) -> ValidationError {
	ValidationError {
		path: path.into(),
		keyword: keyword.into(),
		message: message.into(),
	}
}

fn compile_error(schema: &Value) -> String {
	match compile(schema, &CompileOptions::default()) {
		Ok(_) => panic!("expected compile to throw"),
		Err(error) => error.message,
	}
}

mod errors {
	use super::*;

	#[test]
	fn reports_a_missing_field_and_an_unknown_field_at_the_field() {
		assert_eq!(
			plain(&read()).errors(&json!({ "offset": 0, "extra": true })).unwrap(),
			vec![
				error("/path", "required", "missing"),
				error("/offset", "minimum", "expected >= 1"),
				error("/extra", "additionalProperties", "unknown field"),
			]
		);
	}

	#[test]
	fn is_empty_for_a_valid_value() {
		assert_eq!(plain(&read()).errors(&json!({ "path": "a.ts" })).unwrap(), vec![]);
	}

	#[test]
	fn reports_the_branch_a_tagged_union_s_tag_selects() {
		let validator = plain(&json!({ "type": "array", "items": block() }));
		assert_eq!(
			validator
				.errors(&json!([{ "kind": "text", "text": "a" }, { "kind": "image", "width": -1 }]))
				.unwrap(),
			vec![error("/1/width", "minimum", "expected >= 0")]
		);
		assert_eq!(
			validator.errors(&json!([{ "kind": "video" }])).unwrap(),
			vec![error("/0/kind", "anyOf", r#"expected one of ["text","image"]"#)]
		);
	}

	#[test]
	fn escapes_and_in_paths_as_json_pointer_does() {
		let validator = plain(&json!({ "type": "object", "properties": { "a/b~c": { "type": "string" } } }));
		assert_eq!(
			validator.errors(&json!({ "a/b~c": 1 })).unwrap(),
			vec![error("/a~1b~0c", "type", "expected string, got integer")]
		);
	}

	#[test]
	fn follows_ref_into_the_referenced_schema() {
		let validator = plain(&json!({
			"$defs": { "point": { "type": "object", "properties": { "x": { "type": "number" } }, "required": ["x"] } },
			"type": "array",
			"items": { "$ref": "#/$defs/point" },
		}));
		assert_eq!(
			validator.errors(&json!([{ "x": 1 }, {}])).unwrap(),
			vec![error("/1/x", "required", "missing")]
		);
	}
}

mod values {
	use super::*;

	#[test]
	fn treats_a_property_set_to_undefined_as_absent() {
		// JSON has no undefined: { path: "a.ts", offset: undefined } is { path: "a.ts" }, { path: undefined } is {}.
		let validator = plain(&read());
		assert!(validator.check(&json!({ "path": "a.ts" })));
		assert!(!validator.check(&json!({})));
	}

	#[test]
	fn checks_bytes_as_uint8_array_with_length_bounds() {
		// No JSON value is a Uint8Array: bytes accepts none of them (the TypeScript's Uint8Array cases cannot be built).
		let validator =
			plain(&json!({ "type": "object", "properties": { "data": { "type": "bytes", "maxLength": 4 } } }));
		assert!(!validator.check(&json!({ "data": [1, 2] })));
	}

	#[test]
	#[ignore = "no JSON value is a Uint8Array"]
	fn does_not_treat_bytes_as_an_object() {}

	#[test]
	fn checks_properties_named_like_object_prototype_members_as_own_properties() {
		let validator = plain(&json!({
			"type": "object",
			"properties": { "constructor": { "type": "string" }, "toString": { "type": "number" } },
			"required": ["constructor"],
			"additionalProperties": false,
		}));
		assert!(!validator.check(&json!({})));
		assert!(validator.check(&json!({ "constructor": "a" })));
		assert!(!validator.check(&json!({ "constructor": "a", "toString": "b" })));
	}

	#[test]
	fn does_not_accept_every_value_for_a_type_named_like_an_object_prototype_member() {
		assert!(!plain(&json!({ "type": "constructor" })).check(&json!(1)));
	}

	#[test]
	fn checks_a_detached_check_function() {
		let check = plain(&read()).check;
		let values = [json!({ "path": "a" }), json!({ "path": 1 })];
		let valid: Vec<&Value> = values.iter().filter(|v| check(v)).collect();
		assert_eq!(valid, vec![&json!({ "path": "a" })]);
	}
}

mod options {
	use super::*;

	#[test]
	fn treats_format_as_an_annotation_unless_formats_are_given() {
		let schema = json!({ "type": "string", "format": "email" });
		assert!(plain(&schema).check(&json!("not an email")));
		assert!(!compile(&schema, &with_formats()).unwrap().check(&json!("not an email")));
		assert!(
			compile(&schema, &with_formats())
				.unwrap()
				.check(&json!("a@example.com"))
		);
	}

	#[test]
	fn resolves_ref_to_registered_documents() {
		let point = json!({ "type": "object", "properties": { "x": { "type": "number" } }, "required": ["x"] });
		let options = CompileOptions {
			schemas: Some(std::rc::Rc::new(
				[("https://example.com/point.json".to_string(), point)]
					.into_iter()
					.collect(),
			)),
			..CompileOptions::default()
		};
		let validator = compile(&json!({ "$ref": "https://example.com/point.json" }), &options).unwrap();
		assert!(validator.check(&json!({ "x": 1 })));
		assert!(!validator.check(&json!({})));
	}

	#[test]
	fn throws_for_a_ref_that_names_nothing() {
		assert!(compile_error(&json!({ "$ref": "https://example.com/missing.json" })).contains("Unresolved $ref"));
		assert!(compile_error(&json!({ "$ref": "#/$defs/missing" })).contains("Unresolved $ref"));
	}

	#[test]
	fn applies_draft_7_semantics_ref_overrides_its_siblings() {
		let schema = json!({ "definitions": { "s": { "type": "string" } }, "$ref": "#/definitions/s", "maxLength": 1 });
		let draft7 = CompileOptions {
			draft: Some(Draft::V7),
			..CompileOptions::default()
		};
		assert!(compile(&schema, &draft7).unwrap().check(&json!("ab")));
		assert!(!plain(&schema).check(&json!("ab")));
	}
}

mod schemas {
	use super::*;

	#[test]
	fn compiles_type_box_schemas_as_they_are() {
		// Type.Object({ name: Type.String(), tags: Type.Array(Type.String()) })
		let validator = plain(
			&json!({"type":"object","required":["name","tags"],"properties":{"name":{"type":"string"},"tags":{"type":"array","items":{"type":"string"}}}}),
		);
		assert!(validator.check(&json!({ "name": "a", "tags": [] })));
		assert_eq!(
			validator.errors(&json!({ "name": "a", "tags": [1] })).unwrap(),
			vec![error("/tags/0", "type", "expected string, got integer")]
		);
	}

	#[test]
	fn compiles_deep_frozen_schemas_and_never_modifies_a_schema() {
		let mut schema = block();
		schema["$defs"] = json!({ "s": { "type": "string", "pattern": "^a" } });
		let before = pi_host::js::json_stringify(&schema);
		let validator = plain(&schema);
		assert!(validator.check(&json!({ "kind": "text", "text": "x" })));
		assert_eq!(
			validator
				.errors(&json!({ "kind": "image", "width": -1 }))
				.unwrap()
				.len(),
			1
		);
		assert_eq!(pi_host::js::json_stringify(&schema), before);
	}

	#[test]
	fn compiles_a_recursive_schema() {
		let tree = json!({
			"type": "object",
			"properties": { "value": { "type": "number" }, "children": { "type": "array", "items": { "$ref": "#" } } },
			"required": ["value"],
		});
		let validator = plain(&tree);
		assert!(validator.check(&json!({ "value": 1, "children": [{ "value": 2, "children": [{ "value": 3 }] }] })));
		assert_eq!(
			validator
				.errors(&json!({ "value": 1, "children": [{ "value": 2, "children": [{}] }] }))
				.unwrap(),
			vec![error("/children/0/children/0/value", "required", "missing")]
		);
	}

	#[test]
	fn throws_for_a_malformed_schema() {
		assert!(compile_error(&json!({ "properties": { "a": 1 } })).contains("must be an object or a boolean"));
	}
}

mod unresolved_references {
	use super::*;

	fn dangling() -> Value {
		json!({ "type": "object", "properties": { "t": { "$ref": "#/definitions/Missing" } } })
	}

	fn external() -> Value {
		json!({ "type": "object", "properties": { "t": { "$ref": "https://example.com/schemas/t.json" } } })
	}

	fn reject() -> CompileOptions {
		CompileOptions {
			unresolved_refs: Some(UnresolvedRefs::Reject),
			..CompileOptions::default()
		}
	}

	#[test]
	fn throw_when_the_schema_compiles_by_default() {
		assert!(compile_error(&dangling()).contains("Unresolved $ref #/definitions/Missing"));
		assert!(compile_error(&external()).contains("Unresolved $ref https://example.com/schemas/t.json"));
	}

	#[test]
	fn reject_only_values_at_their_place_with_unresolved_refs_reject() {
		for schema in [dangling(), external()] {
			let validator = compile(&schema, &reject()).unwrap();
			assert!(validator.check(&json!({})));
			assert!(!validator.check(&json!({ "t": 1 })));
		}
		assert_eq!(
			compile(&dangling(), &reject())
				.unwrap()
				.errors(&json!({ "t": 1 }))
				.unwrap(),
			vec![error("/t", "$ref", "unresolved reference #/definitions/Missing")]
		);
	}

	#[test]
	fn leave_values_at_their_place_uncoerced() {
		let validator = compile_coercing(&dangling(), &reject()).unwrap();
		assert_eq!(
			validator.parse(&json!({ "t": "1" })).unwrap(),
			ParseResult::Err(vec![error("/t", "$ref", "unresolved reference #/definitions/Missing")])
		);
	}

	#[test]
	fn reject_a_type_box_ref_without_its_definition() {
		// Type.Object({ a: Type.Ref("A") })
		let schema = json!({"type":"object","required":["a"],"properties":{"a":{"$ref":"A"}}});
		let validator = compile(&schema, &reject()).unwrap();
		assert_eq!(
			validator.errors(&json!({ "a": { "x": "1" } })).unwrap(),
			vec![error("/a", "$ref", "unresolved reference A")]
		);
	}
}

mod string_lengths {
	use super::*;

	#[test]
	fn count_an_unpaired_high_surrogate_as_one_code_point_and_a_pair_as_one() {
		// Rust strings hold no unpaired surrogates; the surrogate-pair cases remain.
		assert!(plain(&json!({ "type": "string", "maxLength": 1 })).check(&json!("😀")));
		assert!(!plain(&json!({ "type": "string", "minLength": 2 })).check(&json!("😀")));
	}
}

mod error_messages {
	use super::*;

	#[test]
	fn list_at_most_20_enum_values() {
		let values: Vec<Value> = (0..5000).map(|i| json!(format!("v{i}"))).collect();
		let validator =
			plain(&json!({ "type": "object", "properties": { "e": { "type": "string", "enum": values } } }));
		assert!(validator.check(&json!({ "e": "v4999" })));
		let errors = validator.errors(&json!({ "e": "v5000" })).unwrap();
		let listed = pi_host::js::json_stringify(&Value::Array(values[..20].to_vec()));
		assert_eq!(
			errors[0].message,
			format!("expected one of {}, ... and 4980 more]", &listed[..listed.len() - 1])
		);
		assert_eq!(
			plain(&json!({ "enum": ["a", "b"] })).errors(&json!("c")).unwrap(),
			vec![error("", "enum", r#"expected one of ["a","b"]"#)]
		);
	}

	#[test]
	fn accept_a_10_000_value_enum() {
		let values: Vec<Value> = (0..10_000).map(|i| json!(format!("member{i}"))).collect();
		assert!(plain(&json!({ "enum": values })).check(&json!("member9999")));
	}

	#[test]
	fn describe_contains_bounds_in_words() {
		let at = |schema: Value| {
			let mut full = json!({ "type": "array" });
			for (k, v) in schema.as_object().unwrap() {
				full[k] = v.clone();
			}
			plain(&full)
				.errors(&json!(["a"]))
				.unwrap()
				.first()
				.map(|e| e.message.clone())
		};
		assert_eq!(
			at(json!({ "contains": { "type": "number" } })).as_deref(),
			Some("expected at least 1 matching item")
		);
		assert_eq!(
			at(json!({ "contains": { "type": "number" }, "minContains": 2 })).as_deref(),
			Some("expected at least 2 matching items")
		);
		assert_eq!(
			at(json!({ "contains": { "type": "string" }, "maxContains": 0, "minContains": 0 })).as_deref(),
			Some("expected at most 0 matching items")
		);
		assert_eq!(
			at(json!({ "contains": { "type": "number" }, "minContains": 1, "maxContains": 3 })).as_deref(),
			Some("expected between 1 and 3 matching items")
		);
	}

	#[test]
	fn blame_a_one_of_failure_on_the_branch_that_declares_most_of_the_object_s_properties() {
		let filter = json!({
			"type": "object",
			"properties": { "where": { "$ref": "#/$defs/F" } },
			"$defs": {
				"F": {
					"oneOf": [
						{
							"type": "object",
							"properties": { "field": { "type": "string" }, "eq": { "type": "string" } },
							"required": ["field", "eq"],
							"additionalProperties": false,
						},
						{
							"type": "object",
							"properties": { "and": { "type": "array", "items": { "$ref": "#/$defs/F" } } },
							"required": ["and"],
							"additionalProperties": false,
						},
					],
				},
			},
		});
		assert_eq!(
			plain(&filter)
				.errors(&json!({ "where": { "and": [{ "field": "a", "eq": 1, "x": 2 }] } }))
				.unwrap(),
			vec![
				error("/where/and/0/eq", "type", "expected string, got integer"),
				error("/where/and/0/x", "additionalProperties", "unknown field"),
			]
		);
	}

	#[test]
	fn explain_one_object_reached_through_the_same_reference_at_two_paths_at_each_path() {
		let schema = json!({
			"type": "object",
			"properties": { "a": { "$ref": "#/$defs/P" }, "b": { "$ref": "#/$defs/P" } },
			"$defs": { "P": { "type": "object", "properties": { "n": { "type": "integer" } } } },
		});
		let shared = json!({ "n": "x" });
		assert_eq!(
			plain(&schema).errors(&json!({ "a": shared, "b": shared })).unwrap(),
			vec![
				error("/a/n", "type", "expected integer, got string"),
				error("/b/n", "type", "expected integer, got string"),
			]
		);
	}
}

mod type_box_s_extra_formats {
	use super::*;

	#[test]
	fn check_url_and_json_pointer_uri_fragment() {
		let url = compile(&json!({ "type": "string", "format": "url" }), &with_formats()).unwrap();
		assert!(url.check(&json!("https://example.com/a?b=1")));
		assert!(!url.check(&json!("not a url")));
		let fragment = compile(
			&json!({ "type": "string", "format": "json-pointer-uri-fragment" }),
			&with_formats(),
		)
		.unwrap();
		assert!(fragment.check(&json!("#/a/b~1c")));
		assert!(!fragment.check(&json!("#/a b")));
		assert!(!fragment.check(&json!("/a")));
	}
}
