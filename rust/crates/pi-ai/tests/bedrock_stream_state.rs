//! Port of `packages/ai/test/bedrock-stream-state.test.ts`.
//!
//! #9265: Bedrock kept `index`, `partialJson` and the encrypted reasoning bytes on the content blocks while streaming,
//! and re-parsed the accumulated tool-call arguments on every delta.
//!
//! Differences from the TS:
//! - The TS mocks `@aws-sdk/client-bedrock-runtime` and yields decoded stream items. Here `bedrock_support::serve`
//!   answers with event stream frames (the redacted bytes as base64, as on the wire), and `AWS_BEARER_TOKEN_BEDROCK` is
//!   set so the client can authenticate.
//! - `expectNoStreamState` checks the keys of the serialized content blocks.
//! - "allocates linearly in the argument text" is left out: it measures the V8 heap through `test/memory-helpers.ts`
//!   (`allocatedBytes`), which has no Rust counterpart.

mod bedrock_support;

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use bedrock_support::{event, model, serve, stream_response};
use pi_ai::api::bedrock_converse_stream::stream;
use pi_ai::types::{AssistantContent, AssistantMessage, AssistantMessageEvent, Model, StopReason, TranscriptContext};
use pi_host::js::json_stringify;
use pi_host::testing::run;
use serde_json::{Value, json};

fn test_model() -> Model {
	model(json!({
		"id": "global.anthropic.claude-test",
		"name": "Claude Test",
		"api": "bedrock-converse-stream",
		"provider": "amazon-bedrock",
		"baseUrl": "https://bedrock-runtime.us-east-1.amazonaws.com",
		"reasoning": false,
		"input": ["text"],
		"cost": { "input": 0, "output": 0, "cacheRead": 0, "cacheWrite": 0 },
		"contextWindow": 200_000,
		"maxTokens": 64_000,
	}))
}

fn context() -> TranscriptContext {
	bedrock_support::context(json!({ "messages": [{ "role": "user", "content": "write", "timestamp": 0 }] }))
}

fn content() -> String {
	(0..20_000)
		.map(|line| format!("const line{line} = \"value {line}\"; // \\ done\n"))
		.collect()
}

/// `writeCall(size)`: the arguments and their JSON text.
fn write_call(size: usize) -> (Value, String) {
	let args = json!({ "path": "/src/file.ts", "content": content()[..size] });
	let text = json_stringify(&args);
	(args, text)
}

/// Encrypted reasoning, then a `write` tool call whose arguments arrive in 32-character deltas.
fn events(text: &str) -> Vec<Vec<u8>> {
	let mut events = vec![
		event("messageStart", json!({ "role": "assistant" })),
		event(
			"contentBlockDelta",
			json!({
				"contentBlockIndex": 0,
				"delta": { "reasoningContent": { "redactedContent": STANDARD.encode([1u8, 2]) } },
			}),
		),
		event(
			"contentBlockDelta",
			json!({
				"contentBlockIndex": 0,
				"delta": { "reasoningContent": { "redactedContent": STANDARD.encode([3u8]) } },
			}),
		),
		event("contentBlockStop", json!({ "contentBlockIndex": 0 })),
		event(
			"contentBlockStart",
			json!({ "contentBlockIndex": 1, "start": { "toolUse": { "toolUseId": "tool_1", "name": "write" } } }),
		),
	];
	let mut index = 0;
	while index < text.len() {
		let end = (index + 32).min(text.len());
		events.push(event(
			"contentBlockDelta",
			json!({ "contentBlockIndex": 1, "delta": { "toolUse": { "input": &text[index..end] } } }),
		));
		index += 32;
	}
	events.push(event("contentBlockStop", json!({ "contentBlockIndex": 1 })));
	events.push(event("messageStop", json!({ "stopReason": "tool_use" })));
	events
}

/// `BLOCK_KEYS[type]`.
fn block_keys(block_type: &str) -> &'static [&'static str] {
	match block_type {
		"text" => &["type", "text", "textSignature"],
		"thinking" => &["type", "thinking", "thinkingSignature", "redacted"],
		"toolCall" => &["type", "id", "name", "arguments", "argumentsError"],
		_ => &[],
	}
}

fn expect_no_stream_state(message: &AssistantMessage) {
	for block in &message.content {
		let block = serde_json::to_value(block).unwrap();
		let block_type = block["type"].as_str().unwrap_or_default();
		for key in block.as_object().unwrap().keys() {
			assert!(block_keys(block_type).contains(&key.as_str()), "{block_type}.{key}");
		}
	}
}

mod bedrock_stream_state {
	use super::*;

	#[test]
	fn keeps_stream_state_off_partial_blocks_and_finishes_the_tool_call_with_its_exact_arguments() {
		run(|host| async move {
			host.set_env("AWS_BEARER_TOKEN_BEDROCK", "tok");
			let (args, text) = write_call(5_000);
			serve(&host, vec![stream_response(&events(&text), &[])]);
			let expected_content = args["content"].as_str().unwrap();
			let mut shown = 0;
			let stream = stream(&test_model(), &context(), None);
			while let Some(event) = stream.next().await {
				let (partial, delta_index) = match &event {
					AssistantMessageEvent::Done { .. } | AssistantMessageEvent::Error { .. } => continue,
					AssistantMessageEvent::ToolcallDelta {
						content_index, partial, ..
					} => (partial, Some(*content_index)),
					AssistantMessageEvent::Start { partial }
					| AssistantMessageEvent::TextStart { partial, .. }
					| AssistantMessageEvent::TextDelta { partial, .. }
					| AssistantMessageEvent::TextEnd { partial, .. }
					| AssistantMessageEvent::ThinkingStart { partial, .. }
					| AssistantMessageEvent::ThinkingDelta { partial, .. }
					| AssistantMessageEvent::ThinkingEnd { partial, .. }
					| AssistantMessageEvent::ToolcallStart { partial, .. }
					| AssistantMessageEvent::ToolcallEnd { partial, .. } => (partial, None),
				};
				let partial = partial.borrow();
				expect_no_stream_state(&partial);
				let Some(content_index) = delta_index else {
					continue;
				};
				let Some(AssistantContent::ToolCall(call)) = partial.content.get(content_index) else {
					continue;
				};
				let Some(Value::String(partial_content)) = call.arguments.get("content") else {
					continue;
				};
				assert!(expected_content.starts_with(partial_content.as_str()));
				assert!(partial_content.len() >= shown);
				shown = partial_content.len();
			}
			let message = stream.result().await.unwrap();
			assert_eq!(message.stop_reason, StopReason::ToolUse, "{:?}", message.error_message);
			expect_no_stream_state(&message);
			assert_eq!(
				serde_json::to_value(&message.content).unwrap(),
				json!([
					{ "type": "thinking", "thinking": "[Reasoning redacted]", "thinkingSignature": "AQID", "redacted": true },
					{ "type": "toolCall", "id": "tool_1", "name": "write", "arguments": args },
				])
			);
		});
	}
}
