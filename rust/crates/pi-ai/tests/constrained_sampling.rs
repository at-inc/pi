//! Port of `packages/ai/test/constrained-sampling.test.ts`.
//!
//! typebox schemas are written as the JSON schema they produce. Not ported: "streams custom tool input in time and
//! memory linear in its length" (#9265), which measures V8 heap allocations through `test/memory-helpers.ts`; Rust has
//! no equivalent of that helper. The `stream.push` spy is a task that consumes the event stream while provider events
//! are processed one at a time (`yielding_events`).

mod openai_responses_support;

use std::cell::RefCell;
use std::rc::Rc;

use indexmap::IndexMap;
use openai_responses_support::{assert_match_object, context, create_output, model, usage, yield_now, yielding_events};
use pi_ai::api::constrained_sampling::{
	GrammarToolInputJsonBuffer, append_grammar_tool_input_json_delta, close_grammar_tool_input_json,
	make_strict_json_schema, resolve_json_schema_strict_sampling,
};
use pi_ai::api::openai_responses_shared::{
	ConvertResponsesMessagesOptions, ConvertResponsesToolsOptions, OpenAIResponsesStreamOptions,
	convert_responses_messages, convert_responses_tools, process_responses_stream,
};
use pi_ai::types::{AssistantContent, AssistantMessageEvent, Model, StopReason, Tool};
use pi_ai::utils::event_stream::AssistantMessageEventStream;
use pi_host::testing::run;
use serde_json::{Value, json};

fn make_model() -> Model {
	model(json!({
		"id": "gpt-test",
		"name": "GPT Test",
		"api": "openai-responses",
		"provider": "openai",
		"baseUrl": "https://api.openai.com/v1",
		"reasoning": false,
		"input": ["text", "image"],
		"cost": { "input": 0, "output": 0, "cacheRead": 0, "cacheWrite": 0 },
		"contextWindow": 128000,
		"maxTokens": 4096,
	}))
}

/// `makeTool(overrides)`: `overrides` is spread over the sample tool literal.
fn make_tool(overrides: Value) -> Tool {
	let mut tool = json!({
		"name": "sample_tool",
		"description": "Sample tool",
		"parameters": {
			"additionalProperties": false,
			"type": "object",
			"properties": { "payload": { "type": "string" } },
			"required": ["payload"],
		},
	});
	for (key, value) in overrides.as_object().expect("overrides are an object") {
		tool[key] = value.clone();
	}
	serde_json::from_value(tool).expect("tool JSON is a Tool")
}

fn tools_options(
	supports_strict_mode: Option<bool>,
	supports_openai_grammar_tools: Option<bool>,
) -> ConvertResponsesToolsOptions {
	ConvertResponsesToolsOptions {
		supports_strict_mode,
		supports_openai_grammar_tools,
		..ConvertResponsesToolsOptions::default()
	}
}

fn grammar_options() -> ConvertResponsesMessagesOptions {
	ConvertResponsesMessagesOptions {
		grammar_tool_input_properties: Some(IndexMap::from([("sample_tool".to_string(), "payload".to_string())])),
		..ConvertResponsesMessagesOptions::default()
	}
}

mod constrained_tool_sampling {
	use super::*;

	#[test]
	fn converts_supported_constraints_and_falls_back_when_unsupported() {
		let converted = convert_responses_tools(
			&[make_tool(
				json!({ "constrainedSampling": { "type": "json_schema", "strict": "prefer" } }),
			)],
			None,
		)
		.unwrap();
		assert_match_object(
			&converted[0],
			&json!({ "type": "function", "name": "sample_tool", "strict": true }),
		);

		let error = convert_responses_tools(
			&[make_tool(
				json!({ "constrainedSampling": { "type": "json_schema", "strict": "require" } }),
			)],
			Some(&tools_options(Some(false), None)),
		)
		.unwrap_err();
		assert!(
			error
				.message
				.contains("Tool \"sample_tool\" requires JSON-schema constrained sampling"),
			"{}",
			error.message
		);

		let grammar_tool = make_tool(
			json!({ "constrainedSampling": { "type": "grammar", "variants": { "openai_lark": "start: /[a-z]+/" } } }),
		);
		let converted = convert_responses_tools(
			std::slice::from_ref(&grammar_tool),
			Some(&tools_options(None, Some(true))),
		)
		.unwrap();
		assert_match_object(
			&converted[0],
			&json!({
				"type": "custom",
				"name": "sample_tool",
				"format": { "type": "grammar", "syntax": "lark", "definition": "start: /[a-z]+/" },
			}),
		);
		let error = convert_responses_tools(
			&[make_tool(
				json!({ "constrainedSampling": { "type": "grammar", "variants": {} } }),
			)],
			Some(&tools_options(None, Some(true))),
		)
		.unwrap_err();
		assert!(
			error.message.contains(
				"Tool \"sample_tool\" cannot use grammar constrained sampling: no supported grammar variant was provided"
			),
			"{}",
			error.message
		);

		let fallback =
			convert_responses_tools(&[grammar_tool], Some(&tools_options(Some(false), Some(false)))).unwrap();
		assert_match_object(&fallback[0], &json!({ "type": "function", "name": "sample_tool" }));
		assert!(fallback[0].get("strict").is_none());

		assert_eq!(
			convert_responses_tools(&[make_tool(json!({ "constrainedSampling": false }))], None).unwrap(),
			convert_responses_tools(&[make_tool(json!({}))], None).unwrap()
		);
	}

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
		let original = parameters.clone();

		let strict = make_strict_json_schema(&parameters, None).unwrap();

		assert_eq!(parameters, original);
		assert!(parameters.get("additionalProperties").is_none());
		assert_eq!(parameters["required"], json!(["path", "metadata"]));
		assert_match_object(
			&strict,
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
						"metadata": { "additionalProperties": { "type": "string" }, "type": "object", "properties": {} },
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
			let mut tool = make_tool(json!({
				"parameters": parameters,
				"constrainedSampling": { "type": "json_schema", "strict": "prefer" },
			}));

			let message = make_strict_json_schema(&parameters, None).unwrap_err().message;
			assert!(message.contains(error), "{message}");
			assert_eq!(resolve_json_schema_strict_sampling(&tool, true, None).unwrap(), None);
			let converted =
				convert_responses_tools(std::slice::from_ref(&tool), Some(&tools_options(Some(true), None))).unwrap();
			assert_match_object(&converted[0], &json!({ "strict": false, "parameters": parameters }));

			tool.constrained_sampling =
				Some(serde_json::from_value(json!({ "type": "json_schema", "strict": "require" })).unwrap());
			let message = resolve_json_schema_strict_sampling(&tool, true, None)
				.unwrap_err()
				.message;
			assert!(message.contains(error), "{message}");
		}
	}

	#[test]
	fn replays_grammar_calls_as_custom_responses_items() {
		run(|_| async {
			let transcript = |arguments: Value| {
				context(json!({
					"messages": [
						{
							"role": "assistant",
							"api": "openai-responses",
							"provider": "openai",
							"model": "gpt-test",
							"content": [{ "type": "toolCall", "id": "call_1|ctc_1", "name": "sample_tool", "arguments": arguments }],
							"usage": usage(),
							"stopReason": "toolUse",
							"timestamp": pi_host::now_ms(),
						},
						{
							"role": "toolResult",
							"toolCallId": "call_1|ctc_1",
							"toolName": "sample_tool",
							"content": [{ "type": "text", "text": "done" }],
							"isError": false,
							"timestamp": pi_host::now_ms(),
						},
					],
				}))
			};
			for invalid_arguments in [json!({}), json!({ "payload": 42 })] {
				let error = convert_responses_messages(
					&make_model(),
					&transcript(invalid_arguments),
					&["openai"],
					Some(&grammar_options()),
				)
				.unwrap_err();
				assert!(
					error
						.message
						.contains("Grammar tool call \"sample_tool\" requires argument \"payload\" to be a string"),
					"{}",
					error.message
				);
			}

			let messages = convert_responses_messages(
				&make_model(),
				&transcript(json!({ "payload": "abc" })),
				&["openai"],
				Some(&grammar_options()),
			)
			.unwrap();

			assert!(messages.contains(&json!({
				"type": "custom_tool_call",
				"id": "ctc_1",
				"call_id": "call_1",
				"name": "sample_tool",
				"input": "abc",
			})));
			assert!(
				messages.contains(&json!({ "type": "custom_tool_call_output", "call_id": "call_1", "output": "done" }))
			);
		});
	}

	// earendil-works/radius#115: a gateway forwards another model's history as a foreign provider.
	#[test]
	fn drops_foreign_item_ids_when_replaying_grammar_calls_as_custom_responses_items() {
		run(|_| async {
			let context = context(json!({
				"messages": [
					{
						"role": "assistant",
						"api": "pi-messages",
						"provider": "radius",
						"model": "gpt-other",
						"content": [{ "type": "toolCall", "id": "call_1|ctc_1", "name": "sample_tool", "arguments": { "payload": "abc" } }],
						"usage": usage(),
						"stopReason": "toolUse",
						"timestamp": pi_host::now_ms(),
					},
					{
						"role": "toolResult",
						"toolCallId": "call_1|ctc_1",
						"toolName": "sample_tool",
						"content": [{ "type": "text", "text": "done" }],
						"isError": false,
						"timestamp": pi_host::now_ms(),
					},
				],
			}));

			let messages =
				convert_responses_messages(&make_model(), &context, &["openai"], Some(&grammar_options())).unwrap();

			let call = messages
				.iter()
				.find(|item| item["type"] == "custom_tool_call")
				.expect("custom_tool_call");
			assert_match_object(
				call,
				&json!({ "type": "custom_tool_call", "call_id": "call_1", "input": "abc" }),
			);
			assert!(call.get("id").is_none());
		});
	}

	#[test]
	fn keeps_grammar_input_json_deltas_append_only() {
		let mut buffer = GrammarToolInputJsonBuffer::default();
		let first = append_grammar_tool_input_json_delta(&mut buffer, "payload", "a\"")
			.unwrap()
			.unwrap_or_default();
		let second = close_grammar_tool_input_json(&mut buffer, "payload", Some("a\"\nb"))
			.unwrap()
			.unwrap_or_default();

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

	#[test]
	fn starts_custom_responses_tool_calls_with_their_initial_input() {
		run(|_| async {
			let model = make_model();
			let output = create_output(&model);
			let stream = AssistantMessageEventStream::new();
			let starts = Rc::new(RefCell::new(Vec::new()));
			let deltas = Rc::new(RefCell::new(Vec::new()));
			let (consumer, starts_target, deltas_target) = (stream.clone(), starts.clone(), deltas.clone());
			pi_host::spawn_local(async move {
				while let Some(event) = consumer.next().await {
					match event {
						AssistantMessageEvent::ToolcallStart { content_index, partial } => {
							if let Some(AssistantContent::ToolCall(block)) = partial.borrow().content.get(content_index)
							{
								starts_target.borrow_mut().push(Value::Object(block.arguments.clone()));
							}
						}
						AssistantMessageEvent::ToolcallDelta { delta, .. } => deltas_target.borrow_mut().push(delta),
						_ => {}
					}
				}
			});
			let events = vec![
				json!({
					"type": "response.output_item.added",
					"output_index": 0,
					"item": { "type": "custom_tool_call", "call_id": "call_1", "id": "ctc_1", "name": "sample_tool", "input": "a" },
				}),
				json!({ "type": "response.custom_tool_call_input.delta", "output_index": 0, "item_id": "ctc_1", "delta": "b" }),
				json!({ "type": "response.custom_tool_call_input.done", "output_index": 0, "item_id": "ctc_1", "input": "abc" }),
				json!({
					"type": "response.output_item.done",
					"output_index": 0,
					"item": { "type": "custom_tool_call", "call_id": "call_1", "id": "ctc_1", "name": "sample_tool", "input": "abc" },
				}),
				json!({
					"type": "response.completed",
					"response": { "status": "completed", "usage": { "input_tokens": 1, "output_tokens": 1, "total_tokens": 2 } },
				}),
			];

			let stream_options = OpenAIResponsesStreamOptions {
				grammar_tool_input_properties: Some(IndexMap::from([(
					"sample_tool".to_string(),
					"payload".to_string(),
				)])),
				..OpenAIResponsesStreamOptions::default()
			};
			process_responses_stream(yielding_events(events), &output, &stream, &model, Some(&stream_options))
				.await
				.unwrap();
			yield_now().await;

			let output = output.borrow();
			assert_eq!(output.stop_reason, StopReason::ToolUse);
			assert_eq!(*starts.borrow(), [json!({ "payload": "a" })]);
			assert_eq!(
				serde_json::to_value(&output.content).unwrap(),
				json!([{ "type": "toolCall", "id": "call_1|ctc_1", "name": "sample_tool", "arguments": { "payload": "abc" } }])
			);
			assert_eq!(
				serde_json::from_str::<Value>(&deltas.borrow().concat()).unwrap(),
				json!({ "payload": "abc" })
			);
		});
	}
}
