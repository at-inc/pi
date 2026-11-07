//! Port of `packages/ai/test/constrained-sampling.test.ts`.
//!
//! TypeBox schemas are written out as the JSON Schema they build.
//!
//! Not ported, because they run through `api/openai-responses-shared.ts` (`w3-openai-responses`):
//! "converts supported constraints and falls back when unsupported", "replays grammar calls as custom Responses
//! items", "drops foreign item ids when replaying grammar calls as custom Responses items", "streams custom tool input
//! in time and memory linear in its length" and "starts custom Responses tool calls with their initial input". The
//! `convertResponsesTools` assertion of "falls back or rejects schemas that cannot be safely converted" is left out
//! for the same reason; its `makeStrictJsonSchema` and `resolveJsonSchemaStrictSampling` assertions are ported.

use pi_ai::api::constrained_sampling::{
	GrammarToolInputJsonBuffer, append_grammar_tool_input_json_delta, close_grammar_tool_input_json,
	make_strict_json_schema, resolve_json_schema_strict_sampling,
};
use pi_ai::types::{ConstrainedSamplingConfig, ConstrainedSamplingStrictness, Tool, ToolConstrainedSampling};
use serde_json::{Value, json};

fn make_tool() -> Tool {
	Tool {
		name: "sample_tool".into(),
		description: "Sample tool".into(),
		parameters: json!({
			"type": "object",
			"properties": { "payload": { "type": "string" } },
			"required": ["payload"],
			"additionalProperties": false,
		}),
		..Tool::default()
	}
}

fn json_schema(strict: ConstrainedSamplingStrictness) -> Option<ToolConstrainedSampling> {
	Some(ToolConstrainedSampling::Config(ConstrainedSamplingConfig::JsonSchema {
		strict,
	}))
}

/// `expect(actual).toMatchObject(expected)` for JSON values.
fn assert_match_object(actual: &Value, expected: &Value) {
	match (actual, expected) {
		(Value::Object(actual), Value::Object(expected)) => {
			for (key, expected) in expected {
				assert_match_object(actual.get(key).unwrap_or_else(|| panic!("missing key {key}")), expected);
			}
		}
		(Value::Array(actual), Value::Array(expected)) => {
			assert_eq!(actual.len(), expected.len());
			for (actual, expected) in actual.iter().zip(expected) {
				assert_match_object(actual, expected);
			}
		}
		_ => assert_eq!(actual, expected),
	}
}

mod constrained_tool_sampling {
	use super::*;

	#[test]
	fn derives_strict_provider_schemas_without_changing_tool_definitions() {
		let parameters = json!({
			"type": "object",
			"properties": {
				"path": { "type": "string" },
				"offset": { "type": "number" },
				"metadata": { "type": "object", "properties": { "enabled": { "type": "boolean" } } },
				"nullable": { "anyOf": [{ "type": "string" }, { "type": "null" }] },
			},
			"required": ["path", "metadata"],
		});

		let strict = make_strict_json_schema(&parameters, None).unwrap();

		assert!(parameters.get("additionalProperties").is_none());
		assert_eq!(parameters["required"], json!(["path", "metadata"]));
		assert_match_object(
			&Value::Object(strict),
			&json!({
				"additionalProperties": false,
				"required": ["path", "offset", "metadata", "nullable"],
				"properties": {
					"offset": { "anyOf": [{ "type": "number" }, { "type": "null" }] },
					"metadata": {
						"additionalProperties": false,
						"required": ["enabled"],
						"properties": { "enabled": { "anyOf": [{ "type": "boolean" }, { "type": "null" }] } },
					},
					"nullable": { "anyOf": [{ "type": "string" }, { "type": "null" }] },
				},
			}),
		);
	}

	#[test]
	fn falls_back_or_rejects_schemas_that_cannot_be_safely_converted() {
		let cases = [
			(
				json!({
					"type": "object",
					"properties": {
						"metadata": { "type": "object", "properties": {}, "additionalProperties": { "type": "string" } },
					},
					"required": ["metadata"],
				}),
				"additionalProperties is unsupported",
			),
			(
				json!({
					"allOf": [
						{ "type": "object", "properties": { "a": { "type": "string" } }, "required": ["a"] },
						{ "type": "object", "properties": { "b": { "type": "number" } }, "required": ["b"] },
					],
				}),
				"allOf schemas are unsupported",
			),
			(
				json!({
					"type": "object",
					"properties": {
						"value": {
							"anyOf": [
								{ "type": "object", "properties": { "nested": { "type": "string" } }, "required": ["nested"] },
								{ "type": "null" },
							],
						},
					},
					"required": ["value"],
				}),
				"object and array unions are unsupported",
			),
			(
				json!({
					"type": "object",
					"properties": { "child": { "$ref": "https://example.com/child.json" } },
					"required": ["child"],
				}),
				"$ref schemas are unsupported",
			),
		];

		for (parameters, error) in cases {
			let mut tool = Tool {
				parameters: parameters.clone(),
				constrained_sampling: json_schema(ConstrainedSamplingStrictness::Prefer),
				..make_tool()
			};

			let thrown = make_strict_json_schema(&parameters, None).unwrap_err();
			assert!(thrown.message.contains(error), "{}", thrown.message);
			assert_eq!(resolve_json_schema_strict_sampling(&tool, true, None).unwrap(), None);

			tool.constrained_sampling = json_schema(ConstrainedSamplingStrictness::Require);
			let thrown = resolve_json_schema_strict_sampling(&tool, true, None).unwrap_err();
			assert!(thrown.message.contains(error), "{}", thrown.message);
		}
	}

	#[test]
	fn keeps_grammar_input_json_deltas_append_only() {
		let mut buffer = GrammarToolInputJsonBuffer::default();
		let first = append_grammar_tool_input_json_delta(&mut buffer, "payload", "a\"")
			.unwrap()
			.unwrap();
		let second = close_grammar_tool_input_json(&mut buffer, "payload", Some("a\"\nb"))
			.unwrap()
			.unwrap();

		assert_eq!(
			serde_json::from_str::<Value>(&format!("{first}{second}")).unwrap(),
			json!({ "payload": "a\"\nb" })
		);
		assert_eq!(
			close_grammar_tool_input_json(&mut buffer, "payload", Some("a\"\nb")).unwrap(),
			None
		);
		assert!(
			close_grammar_tool_input_json(&mut buffer, "payload", Some("changed"))
				.unwrap_err()
				.message
				.contains("grammar tool input for property \"payload\" changed after it was closed")
		);
		assert!(
			append_grammar_tool_input_json_delta(&mut buffer, "payload", "x")
				.unwrap_err()
				.message
				.contains("grammar tool input for property \"payload\" changed after it was closed")
		);
	}

	#[test]
	fn rejects_a_final_grammar_input_that_does_not_continue_the_streamed_input() {
		let mut buffer = GrammarToolInputJsonBuffer::default();
		append_grammar_tool_input_json_delta(&mut buffer, "payload", "abc").unwrap();
		assert!(
			close_grammar_tool_input_json(&mut buffer, "payload", Some("abd"))
				.unwrap_err()
				.message
				.contains("grammar tool input for property \"payload\" changed non-monotonically")
		);
	}
}

mod grammar_input_property {
	use super::*;
	use pi_ai::api::constrained_sampling::resolve_grammar_constrained_sampling;

	fn grammar_tool(parameters: Value) -> Tool {
		serde_json::from_value(json!({
			"name": "grammar_tool",
			"description": "",
			"parameters": parameters,
			"constrainedSampling": { "type": "grammar", "variants": { "openai_lark": "start: /x/" } },
		}))
		.unwrap()
	}

	fn error(parameters: Value) -> String {
		resolve_grammar_constrained_sampling(&grammar_tool(parameters), true)
			.unwrap_err()
			.message
	}

	/// Rust-only: `schema.properties?.[name]` also finds inherited members and array indices, like JS.
	#[test]
	fn reads_properties_entries_like_a_js_property_lookup() {
		assert_eq!(
			error(json!({ "type": "object", "required": ["constructor"], "properties": {} })),
			"Tool \"grammar_tool\" cannot use grammar constrained sampling: grammar constrained sampling property constructor must have type string."
		);
		assert_eq!(
			error(json!({ "type": "object", "required": ["0"], "properties": ["x"] })),
			"Tool \"grammar_tool\" cannot use grammar constrained sampling: grammar constrained sampling property 0 must have type string."
		);
		assert_eq!(
			error(json!({ "type": "object", "required": ["missing"], "properties": {} })),
			"Tool \"grammar_tool\" cannot use grammar constrained sampling: grammar constrained sampling requires a properties entry for missing."
		);
	}
}
