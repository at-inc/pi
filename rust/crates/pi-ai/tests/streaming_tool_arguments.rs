//! Port of the `anthropic-messages` cases of `packages/ai/test/streaming-tool-arguments.test.ts` (#9265).
//!
//! The other `PATHS` entries (openai-completions, openai-responses, mistral-conversations, pi-messages, faux) move
//! with those APIs. Of the `describe.each(PATHS)` cases, two are not ported for any path:
//! - "allocates linearly in the argument text" measures `process.memoryUsage()` deltas; counting allocations in
//!   Rust needs a `#[global_allocator]`, which needs `unsafe` (denied workspace-wide).
//! - "releases the stream, its partial messages and argument snapshots once the caller drops them" checks
//!   `WeakRef`s after a forced GC; Rust frees by ownership, so there is no collector to observe.

use std::rc::Rc;

use futures::StreamExt;
use indexmap::IndexMap;
use pi_ai::api::anthropic_messages::{AnthropicOptions, stream};
use pi_ai::types::{
	AssistantContent, AssistantMessage, AssistantMessageEvent, Model, ProviderRequestOptions, StopReason,
	StreamOptions, TranscriptContext,
};
use pi_ai::utils::event_stream::AssistantMessageEventStream;
use pi_ai::utils::transcript::normalize_context;
use pi_host::js::json_stringify;
use pi_host::testing::run;
use pi_host::{FetchFunction, HttpResponse};
use serde_json::{Value, json};

fn file_content() -> String {
	(0..20_000)
		.map(|line| format!("const line{line} = \"value {line}\"; // \\ done\n"))
		.collect()
}

/// `write` arguments with `size` characters of file content, and their JSON text.
fn write_call(size: usize) -> (Value, String) {
	let args = json!({ "path": "/src/file.ts", "content": &file_content()[..size] });
	let text = json_stringify(&args);
	(args, text)
}

fn deltas_of(text: &str, size: usize) -> Vec<String> {
	text.as_bytes()
		.chunks(size)
		.map(|chunk| String::from_utf8(chunk.to_vec()).unwrap())
		.collect()
}

/// A fetch whose response streams `events` as server-sent events, 16 events per chunk.
fn sse_fetch(events: Vec<String>) -> FetchFunction {
	Rc::new(move |_| {
		let chunks: Vec<_> = events
			.chunks(16)
			.map(|chunk| Ok(bytes::Bytes::from(chunk.concat())))
			.collect();
		let headers = IndexMap::from([("content-type".to_string(), "text/event-stream".to_string())]);
		let mut response = HttpResponse::from_bytes(200, headers, Vec::new());
		response.body = futures::stream::iter(chunks).boxed_local();
		Box::pin(async move { Ok(response) })
	})
}

fn sse(data: Value, event: &str) -> String {
	format!("event: {event}\ndata: {}\n\n", json_stringify(&data))
}

fn anthropic_model() -> Model {
	serde_json::from_value(json!({
		"reasoning": false,
		"input": ["text"],
		"cost": { "input": 0, "output": 0, "cacheRead": 0, "cacheWrite": 0 },
		"contextWindow": 200_000,
		"maxTokens": 64_000,
		"id": "claude-test",
		"name": "c",
		"api": "anthropic-messages",
		"provider": "anthropic",
		"baseUrl": "https://api.anthropic.com",
	}))
	.unwrap()
}

fn write_context() -> TranscriptContext {
	normalize_context(
		&serde_json::from_value(json!({ "messages": [{ "role": "user", "content": "write", "timestamp": 0 }] }))
			.unwrap(),
	)
}

fn anthropic_events(deltas: &[String]) -> Vec<String> {
	let mut events = vec![
		sse(
			json!({
				"type": "message_start",
				"message": { "id": "msg_1", "model": "claude-test", "usage": { "input_tokens": 1, "output_tokens": 1 } },
			}),
			"message_start",
		),
		sse(
			json!({
				"type": "content_block_start",
				"index": 0,
				"content_block": { "type": "tool_use", "id": "toolu_1", "name": "write", "input": {} },
			}),
			"content_block_start",
		),
	];
	for delta in deltas {
		events.push(sse(
			json!({ "type": "content_block_delta", "index": 0, "delta": { "type": "input_json_delta", "partial_json": delta } }),
			"content_block_delta",
		));
	}
	events.push(sse(
		json!({ "type": "content_block_stop", "index": 0 }),
		"content_block_stop",
	));
	events.push(sse(
		json!({ "type": "message_delta", "delta": { "stop_reason": "tool_use" }, "usage": { "output_tokens": 10 } }),
		"message_delta",
	));
	events.push(sse(json!({ "type": "message_stop" }), "message_stop"));
	events
}

fn stream_anthropic(events: Vec<String>) -> AssistantMessageEventStream {
	let base = StreamOptions {
		base: ProviderRequestOptions {
			api_key: Some("sk-ant-test".into()),
			fetch: Some(sse_fetch(events)),
			..ProviderRequestOptions::default()
		},
		..StreamOptions::default()
	};
	let options = AnthropicOptions {
		base,
		..AnthropicOptions::default()
	};
	stream(&anthropic_model(), &write_context(), Some(options))
}

/// Own keys a content block may carry; anything else is stream state leaking into partial messages.
fn block_keys(block_type: &str) -> &'static [&'static str] {
	match block_type {
		"text" => &["type", "text", "textSignature"],
		"thinking" => &["type", "thinking", "thinkingSignature", "redacted"],
		"toolCall" => &[
			"type",
			"id",
			"name",
			"arguments",
			"argumentsError",
			"thoughtSignature",
			"namespace",
		],
		_ => &[],
	}
}

fn expect_no_stream_state(message: &AssistantMessage, label: &str) {
	for block in &message.content {
		let block = serde_json::to_value(block).unwrap();
		let block_type = block["type"].as_str().unwrap().to_string();
		for key in block.as_object().unwrap().keys() {
			assert!(
				block_keys(&block_type).contains(&key.as_str()),
				"{label}: {block_type}.{key}"
			);
		}
	}
}

fn event_partial(event: &AssistantMessageEvent) -> (&'static str, &Rc<std::cell::RefCell<AssistantMessage>>) {
	match event {
		AssistantMessageEvent::Start { partial } => ("start", partial),
		AssistantMessageEvent::TextStart { partial, .. } => ("text_start", partial),
		AssistantMessageEvent::TextDelta { partial, .. } => ("text_delta", partial),
		AssistantMessageEvent::TextEnd { partial, .. } => ("text_end", partial),
		AssistantMessageEvent::ThinkingStart { partial, .. } => ("thinking_start", partial),
		AssistantMessageEvent::ThinkingDelta { partial, .. } => ("thinking_delta", partial),
		AssistantMessageEvent::ThinkingEnd { partial, .. } => ("thinking_end", partial),
		AssistantMessageEvent::ToolcallStart { partial, .. } => ("toolcall_start", partial),
		AssistantMessageEvent::ToolcallDelta { partial, .. } => ("toolcall_delta", partial),
		AssistantMessageEvent::ToolcallEnd { partial, .. } => ("toolcall_end", partial),
		AssistantMessageEvent::Done { message, .. } => ("done", message),
		AssistantMessageEvent::Error { error, .. } => ("error", error),
	}
}

async fn consume(
	events: AssistantMessageEventStream,
	mut on_event: impl FnMut(&AssistantMessageEvent),
) -> AssistantMessage {
	while let Some(event) = events.next().await {
		on_event(&event);
	}
	events.result().await.expect("the stream ends with a message")
}

mod anthropic_messages_tool_call_argument_streaming {
	use super::*;

	#[test]
	fn shows_growing_partial_arguments_keeps_stream_state_off_every_partial_and_finishes_with_the_exact_arguments() {
		run(|_| async {
			let (args, text) = write_call(5_000);
			let full = args["content"].as_str().unwrap().to_string();
			let mut shown: i64 = -1;
			let mut partials = 0;
			let message = consume(stream_anthropic(anthropic_events(&deltas_of(&text, 24))), |event| {
				let (event_type, partial) = event_partial(event);
				let partial = partial.borrow();
				expect_no_stream_state(&partial, event_type);
				let AssistantMessageEvent::ToolcallDelta { content_index, .. } = event else {
					return;
				};
				let Some(AssistantContent::ToolCall(call)) = partial.content.get(*content_index) else {
					return;
				};
				let Some(Value::String(shown_content)) = call.arguments.get("content") else {
					return;
				};
				partials += 1;
				assert!(full.starts_with(shown_content.as_str()));
				assert!(shown_content.len() as i64 >= shown);
				shown = shown_content.len() as i64;
			})
			.await;
			assert_eq!(message.stop_reason, StopReason::ToolUse);
			assert!(partials > 100, "{partials}");
			assert_eq!(message.content.len(), 1);
			let AssistantContent::ToolCall(call) = &message.content[0] else {
				panic!("expected a tool call, got {:?}", message.content[0]);
			};
			assert_eq!(call.name, "write");
			assert_eq!(Value::Object(call.arguments.clone()), args);
			assert_eq!(
				serde_json::to_value(&message.content[0])
					.unwrap()
					.as_object()
					.unwrap()
					.len(),
				4,
				"{{ type, id, name, arguments }}"
			);
		});
	}
}

mod tool_call_stream_state_on_errors {
	use super::*;

	#[test]
	fn leaves_no_stream_state_on_the_error_message_of_a_stream_cut_off_mid_call() {
		run(|_| async {
			let (_, text) = write_call(2_000);
			let cut: Vec<String> = deltas_of(&text, 16).into_iter().take(20).collect();
			let mut events = anthropic_events(&cut);
			// Drop the block stop and the message end, as a dropped connection does.
			events.truncate(events.len() - 3);
			let message = consume(stream_anthropic(events), |_| {}).await;
			assert_eq!(message.stop_reason, StopReason::Error);
			expect_no_stream_state(&message, "error");
			let AssistantContent::ToolCall(call) = &message.content[0] else {
				panic!("expected a tool call, got {:?}", message.content[0]);
			};
			assert_eq!(call.arguments.get("path"), Some(&json!("/src/file.ts")));
		});
	}
}
