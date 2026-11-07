//! Port of `packages/ai/test/streaming-tool-arguments.test.ts` (#9265): the "openai-responses" path.
//!
//! Ported: "shows growing partial arguments, keeps stream state off every partial, and finishes with the exact
//! arguments" for the openai-responses path. A consumer task reads the event stream while the provider events are
//! processed one at a time (`yielding_events`), so each `toolcall_delta` partial is inspected before the next delta
//! lands, as `for await` does in the TS. Content blocks are typed structs, so "no stream state on a partial" is checked
//! as the serialized keys of each block.
//!
//! Not ported: "allocates linearly in the argument text" and "releases the stream, its partial messages and argument
//! snapshots once the caller drops them" measure V8 allocations and `WeakRef` collection through
//! `test/memory-helpers.ts`, which have no Rust equivalent. The anthropic-messages, openai-completions,
//! mistral-conversations and pi-messages paths and the "tool-call stream state on errors" describe (Anthropic) belong
//! to the shards porting those adapters; the faux path to the w2 faux provider port.

mod openai_responses_support;

use std::cell::RefCell;
use std::rc::Rc;

use openai_responses_support::{create_output, model, yield_now, yielding_events};
use pi_ai::api::openai_responses_shared::process_responses_stream;
use pi_ai::types::{AssistantContent, AssistantMessage, AssistantMessageEvent, StopReason};
use pi_ai::utils::event_stream::AssistantMessageEventStream;
use pi_host::testing::run;
use serde_json::{Value, json};

/// `write` arguments with `size` characters of file content, and their JSON text.
fn write_call(size: usize) -> (Value, String) {
	let content: String = (0..20_000)
		.map(|line| format!("const line{line} = \"value {line}\"; // \\ done\n"))
		.collect();
	let args = json!({ "path": "/src/file.ts", "content": content[..size] });
	let text = pi_host::js::json_stringify(&args);
	(args, text)
}

fn deltas_of(text: &str, size: usize) -> Vec<String> {
	text.chars()
		.collect::<Vec<_>>()
		.chunks(size)
		.map(|chunk| chunk.iter().collect())
		.collect()
}

fn responses_events(deltas: &[String], text: &str) -> Vec<Value> {
	let item = json!({
		"type": "function_call",
		"id": "fc_1",
		"call_id": "call_1",
		"name": "write",
		"arguments": "",
		"status": "in_progress",
	});
	let mut events =
		vec![json!({ "type": "response.output_item.added", "output_index": 0, "sequence_number": 0, "item": item })];
	events.extend(deltas.iter().map(|delta| {
		json!({
			"type": "response.function_call_arguments.delta",
			"output_index": 0,
			"item_id": "fc_1",
			"sequence_number": 0,
			"delta": delta,
		})
	}));
	let mut done_item = item.clone();
	done_item["arguments"] = json!(text);
	done_item["status"] = json!("completed");
	events.extend([
		json!({ "type": "response.function_call_arguments.done", "output_index": 0, "item_id": "fc_1", "sequence_number": 0, "arguments": text }),
		json!({ "type": "response.output_item.done", "output_index": 0, "sequence_number": 0, "item": done_item }),
		json!({
			"type": "response.completed",
			"sequence_number": 0,
			"response": {
				"id": "resp_1",
				"status": "completed",
				"output": [],
				"usage": { "input_tokens": 1, "output_tokens": 1, "total_tokens": 2 },
			},
		}),
	]);
	events
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
	for block in serde_json::to_value(&message.content)
		.expect("content serializes")
		.as_array()
		.into_iter()
		.flatten()
	{
		let block_type = block["type"].as_str().unwrap_or_default();
		for key in block.as_object().into_iter().flat_map(|block| block.keys()) {
			assert!(
				block_keys(block_type).contains(&key.as_str()),
				"{label}: {block_type}.{key}"
			);
		}
	}
}

mod openai_responses_tool_call_argument_streaming {
	use super::*;

	#[test]
	fn shows_growing_partial_arguments_keeps_stream_state_off_every_partial_and_finishes_with_the_exact_arguments() {
		run(|_| async {
			let (args, text) = write_call(5_000);
			let model = model(json!({
				"id": "gpt-test",
				"name": "g",
				"api": "openai-responses",
				"provider": "openai",
				"baseUrl": "https://api.openai.com/v1",
				"reasoning": false,
				"input": ["text"],
				"cost": { "input": 0, "output": 0, "cacheRead": 0, "cacheWrite": 0 },
				"contextWindow": 200_000,
				"maxTokens": 64_000,
			}));
			let output = create_output(&model);
			output.borrow_mut().timestamp = 0.0;
			let events = AssistantMessageEventStream::new();
			let shown = Rc::new(RefCell::new((-1_i64, 0_usize)));
			let (consumer, target, expected_content) = (
				events.clone(),
				shown.clone(),
				args["content"].as_str().unwrap_or_default().to_string(),
			);
			pi_host::spawn_local(async move {
				while let Some(event) = consumer.next().await {
					let (label, partial) = match &event {
						AssistantMessageEvent::Done { message, .. } => ("done", message.clone()),
						AssistantMessageEvent::Error { error, .. } => ("error", error.clone()),
						AssistantMessageEvent::Start { partial } => ("start", partial.clone()),
						AssistantMessageEvent::ToolcallStart { partial, .. } => ("toolcall_start", partial.clone()),
						AssistantMessageEvent::ToolcallDelta { partial, .. } => ("toolcall_delta", partial.clone()),
						AssistantMessageEvent::ToolcallEnd { partial, .. } => ("toolcall_end", partial.clone()),
						_ => continue,
					};
					expect_no_stream_state(&partial.borrow(), label);
					let AssistantMessageEvent::ToolcallDelta { content_index, .. } = event else {
						continue;
					};
					let partial = partial.borrow();
					let Some(AssistantContent::ToolCall(call)) = partial.content.get(content_index) else {
						continue;
					};
					let Some(Value::String(shown_content)) = call.arguments.get("content") else {
						continue;
					};
					let mut shown = target.borrow_mut();
					shown.1 += 1;
					assert!(expected_content.starts_with(shown_content.as_str()));
					let length = pi_host::js::utf16_len(shown_content) as i64;
					assert!(length >= shown.0);
					shown.0 = length;
				}
			});

			events.push(AssistantMessageEvent::Start {
				partial: output.clone(),
			});
			process_responses_stream(
				yielding_events(responses_events(&deltas_of(&text, 24), &text)),
				&output,
				&events,
				&model,
				None,
			)
			.await
			.unwrap();
			events.push(AssistantMessageEvent::Done {
				reason: StopReason::ToolUse,
				message: output.clone(),
			});
			events.end(None);
			yield_now().await;

			let message = output.borrow();
			assert_eq!(message.stop_reason, StopReason::ToolUse);
			assert!(shown.borrow().1 > 100, "{} partials", shown.borrow().1);
			let content = serde_json::to_value(&message.content).unwrap();
			assert_eq!(content.as_array().map(Vec::len), Some(1));
			assert!(content[0]["id"].is_string());
			assert_eq!(content[0]["type"], "toolCall");
			assert_eq!(content[0]["name"], "write");
			assert_eq!(content[0]["arguments"], args);
			assert_eq!(content[0].as_object().map(|block| block.len()), Some(4));
		});
	}
}
