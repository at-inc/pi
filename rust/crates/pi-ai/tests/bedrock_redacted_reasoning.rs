//! Port of `packages/ai/test/bedrock-redacted-reasoning.test.ts`.
//!
//! OpenAI models served through Bedrock Converse (e.g. `global.openai.gpt-5.6-terra`)
//! return encrypted reasoning as the opaque `redactedContent` member of
//! `reasoningContent`, not as `reasoningText`. The AWS SDK decodes the wire blob to
//! `Uint8Array`.
//! See https://docs.aws.amazon.com/bedrock/latest/APIReference/API_runtime_ReasoningContentBlockDelta.html
//!
//! Differences from the TS:
//! - The TS mocks `@aws-sdk/client-bedrock-runtime` and yields decoded stream items. Here
//!   `bedrock_support::serve` answers with event stream frames whose JSON payloads carry the blob as base64, as on the
//!   wire, and `AWS_BEARER_TOKEN_BEDROCK` is set so the client can authenticate.
//! - `capturePayload` reads the command input through `onPayload` with an already aborted signal. Blob members of
//!   the command input are base64 text in the port (see `api::bedrock_converse_stream`), so the replayed
//!   `redactedContent` is compared with `redactedBase64` where the TS compares it with `redactedBytes`.
//! - `"redactedChunks" in thinking` and `"index" in thinking` check the keys of the serialized thinking block.

mod bedrock_support;

use std::cell::RefCell;
use std::rc::Rc;

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use bedrock_support::{collect, event, model, serve, stream_response};
use pi_ai::api::bedrock_converse_stream::{BedrockOptions, stream};
use pi_ai::types::{AssistantContent, AssistantMessage, CacheRetention, Model, StopReason, StreamOptions};
use pi_host::AbortController;
use pi_host::testing::{TestHost, run};
use serde_json::{Value, json};

const REDACTED_BASE64: &str = "cnNuXzVaVnJpZjRKMGJYSXFtV2RsZWRqN1FJRmVOaWtSUWJF";

fn redacted_bytes() -> Vec<u8> {
	STANDARD.decode(REDACTED_BASE64).expect("valid base64")
}

fn gpt_model() -> Model {
	model(json!({
		"id": "global.openai.gpt-5.6-terra",
		"name": "GPT-5.6 Terra (Global)",
		"api": "bedrock-converse-stream",
		"provider": "amazon-bedrock",
		"baseUrl": "https://bedrock-runtime.ap-northeast-1.amazonaws.com",
		"reasoning": true,
		"input": ["text"],
		"cost": { "input": 1.25, "output": 10, "cacheRead": 0.125, "cacheWrite": 0 },
		"contextWindow": 400000,
		"maxTokens": 128000,
	}))
}

fn empty_usage() -> Value {
	json!({
		"input": 0,
		"output": 0,
		"cacheRead": 0,
		"cacheWrite": 0,
		"totalTokens": 0,
		"cost": { "input": 0, "output": 0, "cacheRead": 0, "cacheWrite": 0, "total": 0 },
	})
}

/// A `contentBlockDelta` frame carrying `bytes` as `reasoningContent.redactedContent`.
fn redacted_delta(bytes: &[u8]) -> Vec<u8> {
	event(
		"contentBlockDelta",
		json!({
			"contentBlockIndex": 0,
			"delta": { "reasoningContent": { "redactedContent": STANDARD.encode(bytes) } },
		}),
	)
}

/// Mirrors the ConverseStream frames GPT-5.6 emits: encrypted reasoning, then text.
fn redacted_reasoning_events() -> Vec<Vec<u8>> {
	vec![
		event("messageStart", json!({ "role": "assistant" })),
		redacted_delta(&redacted_bytes()),
		event("contentBlockStop", json!({ "contentBlockIndex": 0 })),
		event(
			"contentBlockDelta",
			json!({ "contentBlockIndex": 1, "delta": { "text": "done" } }),
		),
		event("contentBlockStop", json!({ "contentBlockIndex": 1 })),
		event("messageStop", json!({ "stopReason": "end_turn" })),
	]
}

/// `streamBedrock(gptModel, helloContext).result()` against a response of `events`.
async fn stream_hello(host: &TestHost, events: Vec<Vec<u8>>) -> AssistantMessage {
	host.set_env("AWS_BEARER_TOKEN_BEDROCK", "tok");
	serve(host, vec![stream_response(&events, &[])]);
	let hello_context =
		bedrock_support::context(json!({ "messages": [{ "role": "user", "content": "hello", "timestamp": 1 }] }));
	stream(&gpt_model(), &hello_context, None).result().await.unwrap()
}

/// `response.content.find((c) => c.type === "thinking")`, serialized.
fn find_thinking(response: &AssistantMessage) -> Value {
	let thinking = response
		.content
		.iter()
		.find(|c| matches!(c, AssistantContent::Thinking(_)))
		.expect("a thinking block");
	serde_json::to_value(thinking).unwrap()
}

/// `capturePayload(context)`.
async fn capture_payload(messages: Value) -> Value {
	let captured = Rc::new(RefCell::new(None));
	let recorded = captured.clone();
	let controller = AbortController::new();
	controller.abort();
	let mut options = BedrockOptions {
		base: StreamOptions {
			cache_retention: Some(CacheRetention::None),
			..StreamOptions::default()
		},
		..BedrockOptions::default()
	};
	options.signal = Some(controller.signal());
	options.on_payload = Some(Rc::new(move |payload: Value, _: &Model| {
		*recorded.borrow_mut() = Some(payload.clone());
		Box::pin(async move { Ok(Some(payload)) })
	}));
	let context = bedrock_support::context(json!({ "messages": messages }));
	collect(stream(&gpt_model(), &context, Some(options))).await;
	captured
		.take()
		.expect("Expected Bedrock payload to be captured before request abort")
}

/// `payload.messages.find((m) => m.role === "assistant")`.
fn find_assistant(payload: &Value) -> &Value {
	payload["messages"]
		.as_array()
		.unwrap()
		.iter()
		.find(|m| m["role"] == "assistant")
		.expect("an assistant message")
}

mod bedrock_redacted_reasoning {
	use super::*;

	#[test]
	fn does_not_fail_the_stream_when_reasoning_arrives_as_redacted_content() {
		run(|host| async move {
			let response = stream_hello(&host, redacted_reasoning_events()).await;

			assert_ne!(response.stop_reason, StopReason::Error, "{:?}", response.error_message);
			// Reasoning precedes the answer, matching the order Bedrock streamed it.
			let types: Vec<Value> = response
				.content
				.iter()
				.map(|c| serde_json::to_value(c).unwrap()["type"].clone())
				.collect();
			assert_eq!(types, [json!("thinking"), json!("text")]);
			assert_eq!(
				serde_json::to_value(&response.content[1]).unwrap(),
				json!({ "type": "text", "text": "done" })
			);
		});
	}

	#[test]
	fn preserves_the_encrypted_reasoning_payload_on_the_assistant_message() {
		run(|host| async move {
			let response = stream_hello(&host, redacted_reasoning_events()).await;

			let thinking = find_thinking(&response);
			// Same representation Anthropic redacted thinking already uses: the opaque
			// payload rides in `thinkingSignature` with `redacted: true`.
			assert_eq!(thinking["redacted"], json!(true));
			assert_eq!(thinking["thinkingSignature"], REDACTED_BASE64);
			// The byte buffer is streaming scratch state: persisting it would bloat the
			// session, since a Uint8Array serializes to an index-keyed object.
			assert!(thinking.get("redactedChunks").is_none());
		});
	}

	#[test]
	fn encodes_the_payload_when_the_stream_never_sends_content_block_stop() {
		run(|host| async move {
			let response = stream_hello(
				&host,
				vec![
					event("messageStart", json!({ "role": "assistant" })),
					redacted_delta(&redacted_bytes()),
					event("messageStop", json!({ "stopReason": "end_turn" })),
				],
			)
			.await;

			let thinking = find_thinking(&response);
			assert_eq!(thinking["thinkingSignature"], REDACTED_BASE64);
			// Streaming scratch state must not survive into the persisted message.
			assert!(thinking.get("redactedChunks").is_none());
			assert!(thinking.get("index").is_none());
		});
	}

	#[test]
	fn joins_encrypted_reasoning_split_across_deltas() {
		run(|host| async move {
			let bytes = redacted_bytes();
			let (head, tail) = bytes.split_at(7);
			let response = stream_hello(
				&host,
				vec![
					event("messageStart", json!({ "role": "assistant" })),
					redacted_delta(head),
					redacted_delta(tail),
					event("contentBlockStop", json!({ "contentBlockIndex": 0 })),
					event("messageStop", json!({ "stopReason": "end_turn" })),
				],
			)
			.await;

			let thinking = find_thinking(&response);
			assert_eq!(thinking["thinkingSignature"], REDACTED_BASE64);
			// The placeholder marks the block once, not once per delta.
			assert_eq!(thinking["thinking"], "[Reasoning redacted]");
		});
	}

	#[test]
	fn replays_redacted_reasoning_as_reasoning_content_redacted_content() {
		run(|_| async {
			let payload = capture_payload(json!([
				{ "role": "user", "content": "hello", "timestamp": 1 },
				{
					"role": "assistant",
					"content": [
						{ "type": "thinking", "thinking": "", "thinkingSignature": REDACTED_BASE64, "redacted": true },
						{ "type": "text", "text": "done" },
					],
					"api": "bedrock-converse-stream",
					"provider": "amazon-bedrock",
					"model": gpt_model().id,
					"usage": empty_usage(),
					"stopReason": "stop",
					"timestamp": 1,
				},
				{ "role": "user", "content": "continue", "timestamp": 1 },
			]))
			.await;

			assert_eq!(
				find_assistant(&payload)["content"],
				json!([
					{ "reasoningContent": { "redactedContent": REDACTED_BASE64 } },
					{ "text": "done" },
				])
			);
		});
	}

	#[test]
	fn replays_redacted_reasoning_before_the_tool_use_block_it_belongs_to() {
		run(|_| async {
			// Bedrock rejects a tool continuation whose reasoning block is missing or
			// reordered, so the opaque payload must land ahead of the matching toolUse.
			let payload = capture_payload(json!([
				{ "role": "user", "content": "read the file", "timestamp": 1 },
				{
					"role": "assistant",
					"content": [
						{ "type": "thinking", "thinking": "", "thinkingSignature": REDACTED_BASE64, "redacted": true },
						{ "type": "toolCall", "id": "tool-1", "name": "read", "arguments": { "path": "/tmp/a.txt" } },
					],
					"api": "bedrock-converse-stream",
					"provider": "amazon-bedrock",
					"model": gpt_model().id,
					"usage": empty_usage(),
					"stopReason": "toolUse",
					"timestamp": 1,
				},
				{
					"role": "toolResult",
					"toolCallId": "tool-1",
					"toolName": "read",
					"content": [{ "type": "text", "text": "file body" }],
					"isError": false,
					"timestamp": 1,
				},
			]))
			.await;

			assert_eq!(
				find_assistant(&payload)["content"],
				json!([
					{ "reasoningContent": { "redactedContent": REDACTED_BASE64 } },
					{ "toolUse": { "toolUseId": "tool-1", "name": "read", "input": { "path": "/tmp/a.txt" } } },
				])
			);
		});
	}
}
