//! Port of `packages/ai/test/openai-responses-partial-json-cleanup.test.ts`.
//!
//! The `push` spy is replaced by draining the stream's buffered events after `processResponsesStream`. `toBe`
//! identity between the persisted tool call and the `toolcall_end` one is checked as equality: Rust events carry a
//! copy of the block.

mod openai_responses_support;

use openai_responses_support::{create_output, gpt5_mini};
use pi_ai::api::openai_responses_shared::process_responses_stream;
use pi_ai::types::{AssistantContent, AssistantMessageEvent};
use pi_ai::utils::event_stream::AssistantMessageEventStream;
use pi_host::PiResult;
use pi_host::testing::run;
use serde_json::{Value, json};

fn create_function_call_events(arguments_json: &str) -> impl futures::Stream<Item = PiResult<Value>> + Unpin {
	futures::stream::iter(
		[
			json!({
				"type": "response.output_item.added",
				"item": { "type": "function_call", "id": "fc_test", "call_id": "call_test", "name": "edit", "arguments": "" },
			}),
			json!({ "type": "response.function_call_arguments.delta", "delta": "{\"path\":\"README.md\"" }),
			json!({ "type": "response.function_call_arguments.delta", "delta": ",\"content\":\"updated\"}" }),
			json!({ "type": "response.function_call_arguments.done", "arguments": arguments_json }),
			json!({
				"type": "response.output_item.done",
				"item": {
					"type": "function_call",
					"id": "fc_test",
					"call_id": "call_test",
					"name": "edit",
					"arguments": arguments_json,
				},
			}),
			json!({
				"type": "response.completed",
				"sequence_number": 5,
				"response": { "id": "resp_test", "status": "completed" },
			}),
		]
		.map(Ok),
	)
}

mod openai_responses_partial_json_cleanup {
	use super::*;

	#[test]
	fn removes_partial_json_from_persisted_tool_call_blocks_at_output_item_done() {
		run(|_| async {
			let model = gpt5_mini();
			let output = create_output(&model);
			let stream = AssistantMessageEventStream::new();
			let arguments_json = r#"{"path":"README.md","content":"updated"}"#;

			process_responses_stream(
				create_function_call_events(arguments_json),
				&output,
				&stream,
				&model,
				None,
			)
			.await
			.unwrap();

			let content = output.borrow().content.clone();
			assert_eq!(content.len(), 1);
			let AssistantContent::ToolCall(persisted_tool_call) = &content[0] else {
				panic!("Expected toolCall block");
			};
			assert_eq!(
				Value::Object(persisted_tool_call.arguments.clone()),
				json!({ "path": "README.md", "content": "updated" })
			);
			assert!(!persisted_tool_call.extra.contains_key("partialJson"));

			stream.end(None);
			let mut tool_call_end = None;
			while let Some(event) = stream.next().await {
				if let AssistantMessageEvent::ToolcallEnd { tool_call, .. } = event {
					tool_call_end = Some(tool_call);
					break;
				}
			}
			let tool_call_end = tool_call_end.expect("Expected toolcall_end event");
			assert_eq!(&tool_call_end, persisted_tool_call);
			assert!(!tool_call_end.extra.contains_key("partialJson"));
		});
	}

	// #10897
	#[test]
	fn marks_malformed_final_arguments_instead_of_keeping_the_parsed_prefix() {
		run(|_| async {
			let model = gpt5_mini();
			let output = create_output(&model);
			let stream = AssistantMessageEventStream::new();
			let arguments_json = r#"{"path":"README.md","content":"updated","mode":.5}"#;

			process_responses_stream(
				create_function_call_events(arguments_json),
				&output,
				&stream,
				&model,
				None,
			)
			.await
			.unwrap();

			let content = output.borrow().content.clone();
			let AssistantContent::ToolCall(tool_call) = &content[0] else {
				panic!("Expected toolCall block");
			};
			assert!(tool_call.arguments.is_empty());
			assert!(
				tool_call
					.arguments_error
					.as_deref()
					.is_some_and(|error| error.ends_with(&format!("Received arguments:\n{arguments_json}")))
			);
		});
	}
}
