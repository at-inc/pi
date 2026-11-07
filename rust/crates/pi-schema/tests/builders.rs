//! Port of `packages/schema/test/builders.test.ts`: the builders, the JSON Schema they produce and how they compile.
//! The TypeScript's static-type assertions (`expectTypeOf`) have no runtime counterpart and are not ported.

use pi_host::js::json_stringify;
use pi_schema::{
	CompileOptions, ValidationError, arr, bool, bytes, compile, int, lit, map, nul, nullable, num, obj, one_of, opt,
	str, tuple, union, unknown,
};
use serde_json::{Value, json};

fn error(path: &str, keyword: &str, message: &str) -> ValidationError {
	ValidationError {
		path: path.into(),
		keyword: keyword.into(),
		message: message.into(),
	}
}

mod builders {
	use super::*;

	#[test]
	fn produce_plain_json_schema() {
		let schema = obj(
			[
				(
					"path",
					str(json!({ "description": "File path", "minLength": 1 })).into(),
				),
				("offset", opt(int(json!({ "minimum": 1 }))).into()),
				("mode", one_of(vec![json!("read"), json!("write")], Value::Null).into()),
			],
			Value::Null,
		);
		// The key order of JSON.stringify, as the TypeScript produces it.
		assert_eq!(
			json_stringify(&schema),
			r#"{"type":"object","properties":{"path":{"type":"string","description":"File path","minLength":1},"offset":{"type":"integer","minimum":1},"mode":{"enum":["read","write"]}},"required":["path","mode"],"additionalProperties":false}"#
		);
	}

	#[test]
	fn produce_every_leaf() {
		assert_eq!(
			vec![
				str(Value::Null),
				num(Value::Null),
				int(Value::Null),
				bool(Value::Null),
				nul(Value::Null),
				unknown(Value::Null),
				lit(json!(3), Value::Null),
				bytes(json!({ "maxLength": 8 })),
			],
			vec![
				json!({ "type": "string" }),
				json!({ "type": "number" }),
				json!({ "type": "integer" }),
				json!({ "type": "boolean" }),
				json!({ "type": "null" }),
				json!({}),
				json!({ "const": 3 }),
				json!({ "type": "bytes", "maxLength": 8 }),
			]
		);
		assert_eq!(
			json_stringify(&arr(str(Value::Null), json!({ "maxItems": 2 }))),
			r#"{"type":"array","items":{"type":"string"},"maxItems":2}"#
		);
		assert_eq!(
			json_stringify(&map(num(Value::Null), Value::Null)),
			r#"{"type":"object","additionalProperties":{"type":"number"}}"#
		);
		assert_eq!(
			json_stringify(&tuple(vec![str(Value::Null), num(Value::Null)], Value::Null)),
			r#"{"type":"array","prefixItems":[{"type":"string"},{"type":"number"}],"items":false,"minItems":2}"#
		);
		assert_eq!(
			json_stringify(&nullable(str(Value::Null), Value::Null)),
			r#"{"anyOf":[{"type":"string"},{"type":"null"}]}"#
		);
		assert_eq!(
			json_stringify(&obj([("a", str(Value::Null).into())], json!({ "open": true }))),
			r#"{"type":"object","properties":{"a":{"type":"string"}},"required":["a"]}"#
		);
	}

	#[test]
	fn carry_the_static_type_of_valid_values() {
		// Type-level only in the TypeScript; here the union of tagged objects is plain JSON Schema.
		let message = union(
			vec![
				obj(
					[
						("kind", lit(json!("text"), Value::Null).into()),
						("text", str(Value::Null).into()),
					],
					Value::Null,
				),
				obj(
					[
						("kind", lit(json!("image"), Value::Null).into()),
						("url", str(Value::Null).into()),
						("alt", opt(str(Value::Null)).into()),
					],
					Value::Null,
				),
			],
			Value::Null,
		);
		assert_eq!(message["anyOf"][1]["required"], json!(["kind", "url"]));
	}

	#[test]
	fn narrow_values_through_a_compiled_check() {
		let point = obj(
			[
				("x", num(Value::Null).into()),
				("y", num(Value::Null).into()),
				("label", opt(str(Value::Null)).into()),
			],
			Value::Null,
		);
		let value = json!({ "x": 1, "y": 2 });
		let check = compile(&point, &CompileOptions::default()).unwrap().check;
		assert!(check(&value), "expected a point");
		assert_eq!(value["x"].as_f64().unwrap() + value["y"].as_f64().unwrap(), 3.0);
	}

	#[test]
	fn compile_into_checks_with_the_declared_semantics() {
		let validator = compile(
			&obj(
				[
					("name", str(json!({ "minLength": 1 })).into()),
					(
						"tags",
						opt(arr(str(Value::Null), json!({ "uniqueItems": true }))).into(),
					),
				],
				Value::Null,
			),
			&CompileOptions::default(),
		)
		.unwrap();
		assert!(validator.check(&json!({ "name": "a" })));
		assert!(validator.check(&json!({ "name": "a", "tags": ["x", "y"] })));
		assert_eq!(
			validator
				.errors(&json!({ "name": "", "tags": ["x", "x"], "other": 1 }))
				.unwrap(),
			vec![
				error("/name", "minLength", "expected at least 1 characters"),
				error("/tags", "uniqueItems", "expected unique items"),
				error("/other", "additionalProperties", "unknown field"),
			]
		);
	}
}
