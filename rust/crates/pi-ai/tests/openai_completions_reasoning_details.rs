//! Port of `packages/ai/test/openai-completions-reasoning-details.test.ts`.
//!
//! Each mocked `create` call shifts the next chunk set; the port serves the chunk sets to successive requests and
//! reads each request body as the recorded payload.

mod openai_completions_support;

use std::cell::RefCell;
use std::collections::VecDeque;
use std::rc::Rc;

use openai_completions_support::{model, options, sse_response};
use pi_ai::api::openai_completions::stream;
use pi_ai::types::{AssistantContent, AssistantMessage, Context, Message, Model, Tool};
use pi_ai::utils::transcript::normalize_context;
use pi_host::js::json_stringify;
use pi_host::testing::{TestHost, run};
use serde_json::{Value, json};

fn reasoning_detail() -> Value {
	json!({ "type": "reasoning.encrypted", "id": "call_1", "data": "encrypted-signature" })
}

fn signed_reasoning_text_detail() -> Value {
	json!({
		"type": "reasoning.text",
		"text": "I should call the read tool.",
		"signature": "sha256:signed-text",
		"id": "reasoning-text-1",
		"format": "anthropic-claude-v1",
		"index": 0,
	})
}

fn reasoning_summary_detail() -> Value {
	json!({
		"type": "reasoning.summary",
		"summary": "Decided to inspect the requested file.",
		"id": "reasoning-summary-1",
		"format": "anthropic-claude-v1",
		"index": 1,
	})
}

fn read_tool() -> Tool {
	Tool {
		name: "read".into(),
		description: "Read a file".into(),
		parameters: json!({
			"type": "object",
			"properties": { "path": { "type": "string" } },
			"required": ["path"],
		}),
		constrained_sampling: None,
		extra: Default::default(),
	}
}

fn test_model() -> Model {
	let mut model = model("google/gemini-test", "openrouter", "https://openrouter.ai/api/v1");
	model.base.name = "Gemini Test".into();
	model.reasoning = true;
	model.context_window = 100_000.0;
	model
}

fn chunk(delta: Value, finish_reason: Option<&str>) -> Value {
	json!({
		"id": "chatcmpl-test",
		"model": "google/gemini-test",
		"choices": [{ "index": 0, "delta": delta, "finish_reason": finish_reason }],
	})
}

fn tool_call_chunk() -> Value {
	chunk(
		json!({
			"tool_calls": [{
				"index": 0,
				"id": "call_1",
				"type": "function",
				"function": { "name": "read", "arguments": "{\"path\":\"README.md\"}" },
			}],
		}),
		None,
	)
}

/// Serves `chunk_sets` to successive requests; a request past the last set gets an empty stream.
fn serve_chunk_sets(host: &TestHost, chunk_sets: Vec<Vec<Value>>) {
	let chunk_sets = Rc::new(RefCell::new(VecDeque::from(chunk_sets)));
	host.on_fetch(move |_| {
		let chunks = chunk_sets.borrow_mut().pop_front().unwrap_or_default();
		let response = sse_response(&chunks);
		Box::pin(async move { Ok(response) })
	});
}

async fn run_openai_completions_stream(messages: Vec<AssistantMessage>) -> AssistantMessage {
	let context = normalize_context(&Context {
		messages: messages.into_iter().map(Message::Assistant).collect(),
		tools: Some(vec![read_tool()]),
		..Context::default()
	});
	stream(&test_model(), &context, Some(options("test")))
		.result()
		.await
		.unwrap()
}

fn payload(host: &TestHost, index: usize) -> Value {
	host.requests()[index].json()
}

fn assistant_payload(payload: &Value) -> Option<&Value> {
	payload["messages"]
		.as_array()?
		.iter()
		.find(|message| message["role"] == "assistant")
}

fn find_block(message: &AssistantMessage, kind: &str) -> Option<Value> {
	message
		.content
		.iter()
		.map(|block| serde_json::to_value(block).unwrap())
		.find(|block| block["type"] == kind)
}

mod openai_completions_reasoning_details_streaming {
	use super::*;

	#[test]
	fn preserves_reasoning_details_in_the_thinking_signature() {
		run(|host| async move {
			serve_chunk_sets(
				&host,
				vec![
					vec![
						chunk(json!({ "reasoning_details": [reasoning_detail()] }), None),
						tool_call_chunk(),
						chunk(json!({}), Some("tool_calls")),
					],
					vec![chunk(json!({ "content": "ok" }), None), chunk(json!({}), Some("stop"))],
				],
			);

			let assistant_message = run_openai_completions_stream(vec![]).await;
			assert_eq!(
				find_block(&assistant_message, "thinking"),
				Some(json!({
					"type": "thinking",
					"thinking": "",
					"thinkingSignature": json_stringify(&json!([reasoning_detail()])),
				}))
			);
			assert_eq!(
				find_block(&assistant_message, "toolCall"),
				Some(json!({
					"type": "toolCall",
					"id": "call_1",
					"name": "read",
					"arguments": { "path": "README.md" },
				}))
			);

			run_openai_completions_stream(vec![assistant_message]).await;

			assert_eq!(
				assistant_payload(&payload(&host, 1)).map(|message| &message["reasoning_details"]),
				Some(&json!([reasoning_detail()]))
			);
		});
	}

	#[test]
	fn falls_back_to_encrypted_tool_call_signatures_for_older_stored_assistant_messages() {
		run(|host| async move {
			serve_chunk_sets(
				&host,
				vec![
					vec![
						chunk(json!({ "reasoning_details": [reasoning_detail()] }), None),
						tool_call_chunk(),
						chunk(json!({}), Some("tool_calls")),
					],
					vec![chunk(json!({ "content": "ok" }), None), chunk(json!({}), Some("stop"))],
				],
			);

			let mut assistant_message = run_openai_completions_stream(vec![]).await;
			assistant_message
				.content
				.retain(|block| !matches!(block, AssistantContent::Thinking(_)));
			let Some(AssistantContent::ToolCall(tool_call)) = assistant_message
				.content
				.iter_mut()
				.find(|block| matches!(block, AssistantContent::ToolCall(_)))
			else {
				panic!("Expected tool call");
			};
			tool_call.thought_signature = Some(json_stringify(&reasoning_detail()));

			run_openai_completions_stream(vec![assistant_message]).await;

			assert_eq!(
				assistant_payload(&payload(&host, 1)).map(|message| &message["reasoning_details"]),
				Some(&json!([reasoning_detail()]))
			);
		});
	}

	#[test]
	fn preserves_signed_text_and_summary_reasoning_details_in_their_original_sequence() {
		run(|host| async move {
			serve_chunk_sets(
				&host,
				vec![
					vec![
						chunk(
							json!({
								"reasoning": signed_reasoning_text_detail()["text"],
								"reasoning_details": [signed_reasoning_text_detail()],
							}),
							None,
						),
						chunk(
							json!({ "reasoning_details": [reasoning_detail(), reasoning_summary_detail()] }),
							None,
						),
						tool_call_chunk(),
						chunk(json!({}), Some("tool_calls")),
					],
					vec![chunk(json!({ "content": "ok" }), None), chunk(json!({}), Some("stop"))],
				],
			);

			let assistant_message = run_openai_completions_stream(vec![]).await;
			let expected_reasoning_details = json!([
				signed_reasoning_text_detail(),
				reasoning_detail(),
				reasoning_summary_detail()
			]);
			assert_eq!(
				find_block(&assistant_message, "thinking"),
				Some(json!({
					"type": "thinking",
					"thinking": signed_reasoning_text_detail()["text"],
					"thinkingSignature": json_stringify(&expected_reasoning_details),
				}))
			);

			run_openai_completions_stream(vec![assistant_message]).await;

			let payload = payload(&host, 1);
			let payload = assistant_payload(&payload).expect("an assistant message");
			assert_eq!(payload["reasoning_details"], expected_reasoning_details);
			assert_eq!(payload.get("reasoning"), None);
		});
	}

	#[test]
	fn merges_consecutive_text_and_summary_reasoning_details_deltas_before_replay() {
		run(|host| async move {
			let text_delta = json!({ "type": "reasoning.text", "text": "The", "index": 0 });
			let text_delta_with_signature = json!({
				"type": "reasoning.text",
				"text": " user wants the time.",
				"signature": "sha256:text-signature",
				"format": "openai-responses-v1",
				"index": 0,
			});
			let summary_delta = json!({ "type": "reasoning.summary", "summary": "Looked", "index": 0 });
			let summary_delta_with_format = json!({
				"type": "reasoning.summary",
				"summary": " up time.",
				"format": "openai-responses-v1",
				"index": 0,
			});
			let later_summary_delta = json!({
				"type": "reasoning.summary",
				"summary": "After encrypted block.",
				"format": "openai-responses-v1",
				"index": 0,
			});
			let expected_reasoning_details = json!([
				{
					"type": "reasoning.text",
					"text": "The user wants the time.",
					"index": 0,
					"signature": "sha256:text-signature",
					"format": "openai-responses-v1",
				},
				{
					"type": "reasoning.summary",
					"summary": "Looked up time.",
					"index": 0,
					"format": "openai-responses-v1",
				},
				reasoning_detail(),
				later_summary_delta,
			]);

			serve_chunk_sets(
				&host,
				vec![
					vec![
						chunk(json!({ "reasoning_details": [text_delta] }), None),
						chunk(json!({ "reasoning_details": [text_delta_with_signature] }), None),
						chunk(json!({ "reasoning_details": [summary_delta] }), None),
						chunk(json!({ "reasoning_details": [summary_delta_with_format] }), None),
						chunk(json!({ "reasoning_details": [reasoning_detail()] }), None),
						chunk(json!({ "reasoning_details": [later_summary_delta] }), None),
						tool_call_chunk(),
						chunk(json!({}), Some("tool_calls")),
					],
					vec![chunk(json!({ "content": "ok" }), None), chunk(json!({}), Some("stop"))],
				],
			);

			let assistant_message = run_openai_completions_stream(vec![]).await;
			let thinking = find_block(&assistant_message, "thinking").expect("a thinking block");
			assert_eq!(
				thinking,
				json!({
					"type": "thinking",
					"thinking": "",
					"thinkingSignature": json_stringify(&expected_reasoning_details),
				})
			);

			run_openai_completions_stream(vec![assistant_message]).await;

			assert_eq!(
				assistant_payload(&payload(&host, 1)).map(|message| &message["reasoning_details"]),
				Some(&expected_reasoning_details)
			);
		});
	}
}

mod openai_completions_reasoning_and_content_in_one_delta {
	use super::*;

	// https://github.com/earendil-works/pi/issues/10926
	#[test]
	fn places_thinking_before_text_when_one_delta_carries_reasoning_content_and_content() {
		run(|host| async move {
			serve_chunk_sets(
				&host,
				vec![vec![chunk(
					json!({ "role": "assistant", "reasoning_content": "thinking", "content": "answer" }),
					Some("stop"),
				)]],
			);

			let assistant_message = run_openai_completions_stream(vec![]).await;
			assert_eq!(
				serde_json::to_value(&assistant_message.content).unwrap(),
				json!([
					{ "type": "thinking", "thinking": "thinking", "thinkingSignature": "reasoning_content" },
					{ "type": "text", "text": "answer" },
				])
			);
		});
	}

	#[test]
	fn places_thinking_before_text_when_one_delta_carries_reasoning_details_and_content() {
		run(|host| async move {
			serve_chunk_sets(
				&host,
				vec![vec![chunk(
					json!({ "reasoning_details": [reasoning_detail()], "content": "answer" }),
					Some("stop"),
				)]],
			);

			let assistant_message = run_openai_completions_stream(vec![]).await;
			let kinds: Vec<Value> = assistant_message
				.content
				.iter()
				.map(|block| serde_json::to_value(block).unwrap()["type"].clone())
				.collect();
			assert_eq!(kinds, vec![json!("thinking"), json!("text")]);
		});
	}
}
