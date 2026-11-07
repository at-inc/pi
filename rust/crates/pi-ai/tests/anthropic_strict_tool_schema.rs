//! Port of `packages/ai/test/anthropic-strict-tool-schema.test.ts`.
//!
//! Differences from the TS: the Typebox schemas are their JSON Schema literals (as Typebox emits them), and
//! `legacyParameters.properties` / `.required` are read from that literal.

mod anthropic_support;

use std::cell::RefCell;
use std::rc::Rc;

use anthropic_support::context;
use pi_ai::api::anthropic_messages::{AnthropicOptions, stream};
use pi_ai::types::{CacheRetention, Model, StreamOptions};
use pi_host::PiError;
use pi_host::testing::run;
use serde_json::{Map, Value, json};

/// `createModel()`.
fn create_model() -> Model {
	serde_json::from_value(json!({
		"id": "claude-opus-4-8",
		"name": "Claude Opus 4.8",
		"api": "anthropic-messages",
		"provider": "test-anthropic",
		"baseUrl": "http://127.0.0.1:9",
		"reasoning": true,
		"input": ["text"],
		"cost": { "input": 0, "output": 0, "cacheRead": 0, "cacheWrite": 0 },
		"contextWindow": 200000,
		"maxTokens": 32000,
		"compat": { "forceAdaptiveThinking": true, "supportsStrictTools": true },
	}))
	.unwrap()
}

/// `createTool(parameters, constrainedSampling)`.
fn create_tool(parameters: Value, constrained_sampling: Option<Value>) -> Value {
	let mut tool = Map::new();
	tool.insert("name".into(), json!("lookup"));
	tool.insert("description".into(), json!("Look up a value"));
	tool.insert("parameters".into(), parameters);
	if let Some(constrained_sampling) = constrained_sampling {
		tool.insert("constrainedSampling".into(), constrained_sampling);
	}
	Value::Object(tool)
}

/// `createStrictTool(parameters)`.
fn create_strict_tool(parameters: Value) -> Value {
	create_tool(parameters, Some(json!({ "type": "json_schema", "strict": "prefer" })))
}

/// `captureFirstTool(tool)`.
async fn capture_first_tool(tool: Value) -> Value {
	let payload: Rc<RefCell<Option<Value>>> = Rc::default();
	let recorder = payload.clone();
	let mut options = AnthropicOptions {
		base: StreamOptions {
			cache_retention: Some(CacheRetention::None),
			..StreamOptions::default()
		},
		..AnthropicOptions::default()
	};
	options.api_key = Some("test-key".into());
	options.on_payload = Some(Rc::new(move |value, _| {
		*recorder.borrow_mut() = Some(value);
		Box::pin(async { Err(PiError::named("PayloadCaptured", "payload captured")) })
	}));
	let transcript = context(json!({
		"messages": [{ "role": "user", "content": "Use the tool", "timestamp": 1 }],
		"tools": [tool],
	}));
	stream(&create_model(), &transcript, Some(options)).result().await;

	let payload = payload.borrow_mut().take();
	payload
		.as_ref()
		.and_then(|payload| payload.get("tools"))
		.and_then(|tools| tools.get(0))
		.cloned()
		.expect("Expected a tool in the captured Anthropic payload")
}

/// `expect(actual).toMatchObject(expected)`: every property of `expected` matches recursively; arrays match element
/// by element with the same length.
fn matches_object(actual: &Value, expected: &Value) -> bool {
	match (actual, expected) {
		(Value::Object(actual), Value::Object(expected)) => expected
			.iter()
			.all(|(key, expected)| actual.get(key).is_some_and(|actual| matches_object(actual, expected))),
		(Value::Array(actual), Value::Array(expected)) => {
			actual.len() == expected.len()
				&& actual
					.iter()
					.zip(expected)
					.all(|(actual, expected)| matches_object(actual, expected))
		}
		(Value::Number(actual), Value::Number(expected)) => actual.as_f64() == expected.as_f64(),
		(actual, expected) => actual == expected,
	}
}

fn assert_match_object(actual: &Value, expected: Value) {
	assert!(matches_object(actual, &expected), "{actual} does not match {expected}");
}

mod anthropic_strict_tool_schemas {
	use super::*;

	#[test]
	fn only_sends_the_full_input_schema_for_strict_json_schema_tools() {
		run(|_| async {
			// Type.Object({ value: Type.String() }, { additionalProperties: false, title: "LookupInput" })
			let legacy_parameters = json!({
				"type": "object",
				"required": ["value"],
				"properties": { "value": { "type": "string" } },
				"additionalProperties": false,
				"title": "LookupInput",
			});
			let legacy_tool = capture_first_tool(create_tool(legacy_parameters.clone(), None)).await;
			assert_eq!(legacy_tool.get("strict"), None);
			assert_eq!(
				legacy_tool["input_schema"],
				json!({
					"type": "object",
					"properties": legacy_parameters["properties"],
					"required": legacy_parameters["required"],
				})
			);

			// Type.Object({ value: Type.String(), optional: Type.Optional(Type.Number()) }, { title: "StrictLookupInput" })
			let strict_tool = capture_first_tool(create_strict_tool(json!({
				"type": "object",
				"required": ["value"],
				"properties": { "value": { "type": "string" }, "optional": { "type": "number" } },
				"title": "StrictLookupInput",
			})))
			.await;
			assert_eq!(strict_tool.get("strict"), Some(&json!(true)));
			assert_match_object(
				&strict_tool["input_schema"],
				json!({
					"additionalProperties": false,
					"required": ["value", "optional"],
					"properties": { "optional": { "anyOf": [{ "type": "number" }, { "type": "null" }] } },
					"title": "StrictLookupInput",
				}),
			);
		});
	}

	// https://github.com/earendil-works/pi/issues/9953
	#[test]
	fn sends_prefer_tools_non_strict_when_they_use_keywords_anthropic_strict_mode_rejects() {
		run(|_| async {
			let unsupported_parameters = [
				// Type.Object({ timeoutMs: Type.Optional(Type.Integer({ minimum: 1, maximum: 300000 })) })
				json!({
					"type": "object",
					"properties": { "timeoutMs": { "type": "integer", "minimum": 1, "maximum": 300000 } },
				}),
				// Type.Object({ options: Type.Object({ tags: Type.Array(Type.String(), { minItems: 2 }) }) })
				json!({
					"type": "object",
					"required": ["options"],
					"properties": {
						"options": {
							"type": "object",
							"required": ["tags"],
							"properties": { "tags": { "type": "array", "items": { "type": "string" }, "minItems": 2 } },
						},
					},
				}),
				// Type.Object({ expression: Type.String({ format: "regex" }) })
				json!({
					"type": "object",
					"required": ["expression"],
					"properties": { "expression": { "type": "string", "format": "regex" } },
				}),
			];
			for parameters in unsupported_parameters {
				let tool = capture_first_tool(create_strict_tool(parameters)).await;
				assert_eq!(tool.get("strict"), None);
			}

			// Type.Object({
			// 	code: Type.String({ minLength: 1, maxLength: 1000, pattern: "^[a-z]+$" }),
			// 	url: Type.String({ format: "uri" }),
			// 	tags: Type.Array(Type.String(), { minItems: 1 }),
			// })
			let supported_tool = capture_first_tool(create_strict_tool(json!({
				"type": "object",
				"required": ["code", "url", "tags"],
				"properties": {
					"code": { "type": "string", "minLength": 1, "maxLength": 1000, "pattern": "^[a-z]+$" },
					"url": { "type": "string", "format": "uri" },
					"tags": { "type": "array", "items": { "type": "string" }, "minItems": 1 },
				},
			})))
			.await;
			assert_eq!(supported_tool.get("strict"), Some(&json!(true)));
		});
	}
}
