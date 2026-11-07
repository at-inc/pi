//! Port of `packages/ai/test/validation.test.ts`.
//!
//! TypeBox schemas are their JSON, as TypeBox 1.3.27 builds them (`Type.Object` lists `required` before
//! `properties`). "still validates when Function constructor is unavailable" has nothing to stub in Rust, which never
//! generates code; it checks the same coercion.

use pi_ai::types::{Tool, ToolCall};
use pi_ai::utils::validation::validate_tool_arguments;
use serde_json::{Value, json};

fn tool(parameters: Value) -> Tool {
	Tool {
		name: "echo".into(),
		description: "Echo tool".into(),
		parameters,
		..Tool::default()
	}
}

fn tool_call(arguments: Value) -> ToolCall {
	ToolCall {
		id: "tool-1".into(),
		name: "echo".into(),
		arguments: arguments.as_object().expect("arguments are an object").clone(),
		..ToolCall::default()
	}
}

fn create_tool_call_with_plain_schema(schema: Value, value: Value) -> (Tool, ToolCall) {
	let tool = tool(json!({
		"type": "object",
		"properties": {
			"value": schema,
		},
		"required": ["value"],
	}));

	let tool_call = tool_call(json!({ "value": value }));

	(tool, tool_call)
}

fn error_message(tool: &Tool, tool_call: &ToolCall) -> String {
	validate_tool_arguments(tool, tool_call)
		.expect_err("validation fails")
		.message
}

mod validate_tool_arguments {
	use super::*;

	#[test]
	fn still_validates_when_function_constructor_is_unavailable() {
		let tool =
			tool(json!({ "type": "object", "required": ["count"], "properties": { "count": { "type": "number" } } }));
		let tool_call = tool_call(json!({ "count": "42" }));

		assert_eq!(
			validate_tool_arguments(&tool, &tool_call).unwrap(),
			json!({ "count": 42 })
		);
	}

	#[test]
	fn coerces_serialized_plain_json_schemas_like_typebox_schemas() {
		let passing_cases = [
			(json!({ "type": "number" }), json!("42"), json!(42)),
			(json!({ "type": "number" }), json!(true), json!(1)),
			(json!({ "type": "number" }), json!(null), json!(0)),
			(json!({ "type": "integer" }), json!("42"), json!(42)),
			(json!({ "type": "boolean" }), json!("true"), json!(true)),
			(json!({ "type": "boolean" }), json!("false"), json!(false)),
			(json!({ "type": "boolean" }), json!(1), json!(true)),
			(json!({ "type": "boolean" }), json!(0), json!(false)),
			// TypeBox's conversion rules come first: null is "null" for a required string.
			(json!({ "type": "string" }), json!(null), json!("null")),
			(json!({ "type": "string" }), json!(true), json!("true")),
			(json!({ "type": "null" }), json!(""), json!(null)),
			(json!({ "type": "null" }), json!(0), json!(null)),
			(json!({ "type": "null" }), json!(false), json!(null)),
			(json!({ "type": ["number", "string"] }), json!("1"), json!("1")),
			(json!({ "type": ["boolean", "number"] }), json!("1"), json!(1)),
			(json!({ "type": "boolean" }), json!("1"), json!(true)),
			(json!({ "type": "boolean" }), json!("0"), json!(false)),
			(json!({ "type": "null" }), json!("null"), json!(null)),
			(json!({ "type": "integer" }), json!("42.1"), json!(42)),
			(
				json!({ "type": "array", "items": { "type": "number" } }),
				json!("7"),
				json!([7]),
			),
		];

		for (schema, input, expected) in passing_cases {
			let (tool, tool_call) = create_tool_call_with_plain_schema(schema.clone(), input.clone());
			assert_eq!(
				validate_tool_arguments(&tool, &tool_call).unwrap(),
				json!({ "value": expected }),
				"{schema} {input}"
			);
		}
	}

	#[test]
	fn treats_null_as_omission_for_optional_non_nullable_properties() {
		let tool = tool(json!({
			"type": "object",
			"required": ["path", "metadata"],
			"properties": {
				"path": { "type": "string" },
				"offset": { "type": "number" },
				"nullable": { "anyOf": [{ "type": "string" }, { "type": "null" }] },
				"metadata": { "type": "object", "properties": { "enabled": { "type": "boolean" } } },
			},
		}));
		let tool_call =
			tool_call(json!({ "path": "file.txt", "offset": null, "nullable": null, "metadata": { "enabled": null } }));

		assert_eq!(
			validate_tool_arguments(&tool, &tool_call).unwrap(),
			json!({
				"path": "file.txt",
				"nullable": null,
				"metadata": {},
			})
		);
	}

	#[test]
	fn preserves_optional_nulls_whose_referenced_schema_is_nullable() {
		let tool = tool(json!({
			"type": "object",
			"properties": { "value": { "$ref": "#/$defs/value" } },
			"$defs": { "value": { "anyOf": [{ "type": "number" }, { "type": "null" }] } },
		}));
		let tool_call = tool_call(json!({ "value": null }));

		assert_eq!(
			validate_tool_arguments(&tool, &tool_call).unwrap(),
			json!({ "value": null })
		);
	}

	#[test]
	fn preserves_a_value_that_already_matches_a_nullable_union_arm() {
		let tool = tool(json!({
			"type": "object",
			"required": ["value"],
			"properties": { "value": { "anyOf": [{ "type": "number" }, { "type": "null" }] } },
		}));
		let tool_call = tool_call(json!({ "value": null }));

		assert_eq!(
			validate_tool_arguments(&tool, &tool_call).unwrap(),
			json!({ "value": null })
		);
	}

	#[test]
	fn preserves_a_value_that_already_matches_a_one_of_nullable_union_arm() {
		let (tool, tool_call) = create_tool_call_with_plain_schema(
			json!({ "oneOf": [{ "type": "number" }, { "type": "null" }] }),
			json!(null),
		);

		assert_eq!(
			validate_tool_arguments(&tool, &tool_call).unwrap(),
			json!({ "value": null })
		);
	}

	#[test]
	fn still_coerces_nullable_unions_when_the_original_value_does_not_match_any_arm() {
		let (tool, tool_call) = create_tool_call_with_plain_schema(
			json!({ "anyOf": [{ "type": "number" }, { "type": "null" }] }),
			json!("42"),
		);

		assert_eq!(
			validate_tool_arguments(&tool, &tool_call).unwrap(),
			json!({ "value": 42 })
		);
	}

	#[test]
	fn accepts_null_for_nullable_array_schemas_with_items() {
		let (tool, tool_call) = create_tool_call_with_plain_schema(
			json!({ "type": ["array", "null"], "items": { "type": "string" } }),
			json!(null),
		);
		assert_eq!(
			validate_tool_arguments(&tool, &tool_call).unwrap(),
			json!({ "value": null })
		);
	}

	#[test]
	fn rejects_invalid_coercions_for_serialized_plain_json_schemas() {
		let failing_cases = [
			(json!({ "type": "boolean" }), json!("yes")),
			(json!({ "type": "number" }), json!("abc")),
			(json!({ "type": "null" }), json!("nil")),
			(json!({ "type": "integer" }), json!("4x")),
		];

		for (schema, input) in failing_cases {
			let (tool, tool_call) = create_tool_call_with_plain_schema(schema.clone(), input.clone());
			assert!(
				error_message(&tool, &tool_call).contains("Validation failed"),
				"{schema} {input}"
			);
		}
	}

	#[test]
	fn lists_pi_schemas_errors_then_the_arguments_it_received() {
		let (tool, tool_call) = create_tool_call_with_plain_schema(json!({ "type": "number" }), json!("many"));
		assert!(error_message(&tool, &tool_call).contains(
			"Validation failed for tool \"echo\":\n  - value: expected number, got string\n\nReceived arguments:\n{\n  \"value\": \"many\"\n}"
		));
	}

	#[test]
	fn reports_a_missing_required_property_at_the_property() {
		let (tool, mut tool_call) = create_tool_call_with_plain_schema(json!({ "type": "string" }), json!("x"));
		tool_call.arguments.clear();
		assert!(error_message(&tool, &tool_call).contains("  - value: missing"));
	}

	#[test]
	fn returns_a_copy_of_the_arguments_also_when_nothing_is_coerced() {
		let (tool, tool_call) = create_tool_call_with_plain_schema(json!({ "type": "string" }), json!("x"));
		let args = validate_tool_arguments(&tool, &tool_call).unwrap();
		assert_eq!(args, json!({ "value": "x" }));
		// Rust: `args` is an owned value, so it is a copy by construction.
		assert_eq!(tool_call.arguments, *json!({ "value": "x" }).as_object().unwrap());
	}

	// #10897
	#[test]
	fn reports_malformed_arguments_json_instead_of_validating_empty_arguments() {
		let tool = tool(json!({ "type": "object", "properties": { "value": { "type": "string" } } }));
		let tool_call = ToolCall {
			arguments_error: Some("Unexpected token".into()),
			..tool_call(json!({}))
		};

		assert!(error_message(&tool, &tool_call).contains("Invalid arguments for tool \"echo\": Unexpected token"));
	}
}
