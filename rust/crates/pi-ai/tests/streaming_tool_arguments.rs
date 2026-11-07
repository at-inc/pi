//! Port of `packages/ai/test/streaming-tool-arguments.test.ts` (#9265).
//!
//! Each `PATHS` entry is a module; all but openai-responses share the case body in
//! `expect_growing_partial_arguments`. The `sseFetch` response is built from the full event list up front instead of
//! lazily. The openai-responses path feeds `process_responses_stream` one event at a time (`yielding_events`) while a consumer task
//! reads the event stream, so each `toolcall_delta` partial is inspected before the next delta lands, as `for await`
//! does in the TS. Of the `describe.each(PATHS)` cases, two are not ported for any path:
//! - "allocates linearly in the argument text" measures `process.memoryUsage()` deltas; counting allocations in
//!   Rust needs a `#[global_allocator]`, which needs `unsafe` (denied workspace-wide).
//! - "releases the stream, its partial messages and argument snapshots once the caller drops them" checks
//!   `WeakRef`s after a forced GC; Rust frees by ownership, so there is no collector to observe.

mod openai_responses_support;

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

/// `sse(data)`: a server-sent event without an event name.
fn sse_data(data: Value) -> String {
	format!("data: {}\n\n", json_stringify(&data))
}

/// `{ ...baseModel, ...fields }`.
fn path_model(fields: Value) -> Model {
	let mut model = json!({
		"reasoning": false,
		"input": ["text"],
		"cost": { "input": 0, "output": 0, "cacheRead": 0, "cacheWrite": 0 },
		"contextWindow": 200_000,
		"maxTokens": 64_000,
	});
	model
		.as_object_mut()
		.unwrap()
		.extend(fields.as_object().unwrap().clone());
	serde_json::from_value(model).unwrap()
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

/// `chatEvents(deltas, withIdEveryChunk)`.
fn chat_events(deltas: &[String], with_id_every_chunk: bool) -> Vec<String> {
	let call = |fields: Value| {
		let mut tool_call = json!({ "index": 0 });
		tool_call
			.as_object_mut()
			.unwrap()
			.extend(fields.as_object().unwrap().clone());
		sse_data(json!({
			"id": "chat_1",
			"object": "chat.completion.chunk",
			"created": 1,
			"model": "test",
			"choices": [{ "index": 0, "delta": { "tool_calls": [tool_call] } }],
		}))
	};
	let mut events = vec![call(
		json!({ "id": "call_1", "type": "function", "function": { "name": "write", "arguments": "" } }),
	)];
	events.extend(deltas.iter().map(|delta| {
		call(if with_id_every_chunk {
			json!({ "id": "call_1", "function": { "name": "write", "arguments": delta } })
		} else {
			json!({ "function": { "arguments": delta } })
		})
	}));
	events.push(sse_data(json!({
		"id": "chat_1",
		"object": "chat.completion.chunk",
		"created": 1,
		"model": "test",
		"choices": [{ "index": 0, "delta": {}, "finish_reason": "tool_calls" }],
		"usage": { "prompt_tokens": 1, "completion_tokens": 1, "total_tokens": 2 },
	})));
	events.push("data: [DONE]\n\n".to_string());
	events
}

/// `piMessagesEvents(deltas, args)`.
fn pi_messages_events(deltas: &[String], args: &Value) -> Vec<String> {
	let mut events = vec![
		sse_data(json!({ "type": "start" })),
		sse_data(json!({ "type": "toolcall_start", "contentIndex": 0, "id": "call_1", "toolName": "write" })),
	];
	events.extend(
		deltas
			.iter()
			.map(|delta| sse_data(json!({ "type": "toolcall_delta", "contentIndex": 0, "delta": delta }))),
	);
	events.push(sse_data(json!({
		"type": "toolcall_end",
		"contentIndex": 0,
		"toolCall": { "type": "toolCall", "id": "call_1", "name": "write", "arguments": args },
	})));
	events.push(sse_data(json!({
		"type": "done",
		"reason": "toolUse",
		"usage": {
			"input": 1,
			"output": 1,
			"cacheRead": 0,
			"cacheWrite": 0,
			"totalTokens": 2,
			"cost": { "input": 0, "output": 0, "cacheRead": 0, "cacheWrite": 0, "total": 0 },
		},
	})));
	events
}

/// `{ apiKey, fetch: sseFetch(() => events) }`.
fn sse_options(api_key: &str, events: Vec<String>) -> StreamOptions {
	StreamOptions {
		base: ProviderRequestOptions {
			api_key: Some(api_key.into()),
			fetch: Some(sse_fetch(events)),
			..ProviderRequestOptions::default()
		},
		..StreamOptions::default()
	}
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

/// The `describe.each(PATHS)` case body: every partial is free of stream state, the shown `content` argument only grows
/// as a prefix of `args.content`, and the final message holds exactly one `write` call with `args`.
async fn expect_growing_partial_arguments(events: AssistantMessageEventStream, args: &Value) {
	let full = args["content"].as_str().unwrap().to_string();
	let mut shown: i64 = -1;
	let mut partials = 0;
	let message = consume(events, |event| {
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
	assert_eq!(&Value::Object(call.arguments.clone()), args);
	assert_eq!(
		serde_json::to_value(&message.content[0])
			.unwrap()
			.as_object()
			.unwrap()
			.len(),
		4,
		"{{ type, id, name, arguments }}"
	);
}

mod anthropic_messages_tool_call_argument_streaming {
	use super::*;

	#[test]
	fn shows_growing_partial_arguments_keeps_stream_state_off_every_partial_and_finishes_with_the_exact_arguments() {
		run(|_| async {
			let (args, text) = write_call(5_000);
			expect_growing_partial_arguments(stream_anthropic(anthropic_events(&deltas_of(&text, 24))), &args).await;
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

mod openai_responses_tool_call_argument_streaming {
	use std::cell::RefCell;

	use super::openai_responses_support::{create_output, model, yield_now, yielding_events};
	use super::*;
	use pi_ai::api::openai_responses_shared::process_responses_stream;

	fn responses_events(deltas: &[String], text: &str) -> Vec<Value> {
		let item = json!({
			"type": "function_call",
			"id": "fc_1",
			"call_id": "call_1",
			"name": "write",
			"arguments": "",
			"status": "in_progress",
		});
		let mut events = vec![
			json!({ "type": "response.output_item.added", "output_index": 0, "sequence_number": 0, "item": item }),
		];
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

mod openai_completions_tool_call_argument_streaming {
	use super::*;
	use pi_ai::api::openai_completions::{OpenAICompletionsOptions, stream};

	#[test]
	fn shows_growing_partial_arguments_keeps_stream_state_off_every_partial_and_finishes_with_the_exact_arguments() {
		run(|_| async {
			let (args, text) = write_call(5_000);
			let model = path_model(json!({
				"id": "gpt-test",
				"name": "g",
				"api": "openai-completions",
				"provider": "openai",
				"baseUrl": "https://api.openai.com/v1",
			}));
			let options = OpenAICompletionsOptions {
				base: sse_options("sk-test", chat_events(&deltas_of(&text, 24), false)),
				..OpenAICompletionsOptions::default()
			};
			expect_growing_partial_arguments(stream(&model, &write_context(), Some(options)), &args).await;
		});
	}
}

mod mistral_conversations_tool_call_argument_streaming {
	use super::*;
	use pi_ai::api::mistral_conversations::{MistralOptions, stream};

	#[test]
	fn shows_growing_partial_arguments_keeps_stream_state_off_every_partial_and_finishes_with_the_exact_arguments() {
		run(|_| async {
			let (args, text) = write_call(5_000);
			let model = path_model(json!({
				"id": "devstral",
				"name": "d",
				"api": "mistral-conversations",
				"provider": "mistral",
				"baseUrl": "https://api.mistral.ai",
			}));
			let options = MistralOptions {
				base: sse_options("test", chat_events(&deltas_of(&text, 24), true)),
				..MistralOptions::default()
			};
			expect_growing_partial_arguments(stream(&model, &write_context(), Some(options)), &args).await;
		});
	}
}

mod pi_messages_tool_call_argument_streaming {
	use super::*;
	use pi_ai::api::pi_messages::{PiMessagesOptions, stream};

	#[test]
	fn shows_growing_partial_arguments_keeps_stream_state_off_every_partial_and_finishes_with_the_exact_arguments() {
		run(|_| async {
			let (args, text) = write_call(5_000);
			let model = path_model(json!({
				"id": "auto",
				"name": "a",
				"api": "pi-messages",
				"provider": "radius",
				"baseUrl": "https://example.test/v1",
			}));
			let options = PiMessagesOptions {
				base: sse_options("test", pi_messages_events(&deltas_of(&text, 24), &args)),
				..PiMessagesOptions::default()
			};
			expect_growing_partial_arguments(stream(&model, &write_context(), Some(options)), &args).await;
		});
	}
}

mod faux_tool_call_argument_streaming {
	use super::*;
	use pi_ai::providers::faux::{
		FauxAssistantMessageOptions, FauxTokenSize, FauxToolCallOptions, RegisterFauxProviderOptions,
		faux_assistant_message, faux_provider, faux_tool_call,
	};
	use pi_ai::types::SimpleStreamOptions;

	#[test]
	fn shows_growing_partial_arguments_keeps_stream_state_off_every_partial_and_finishes_with_the_exact_arguments() {
		run(|_| async {
			let (args, _) = write_call(5_000);
			let faux = faux_provider(Some(RegisterFauxProviderOptions {
				token_size: Some(FauxTokenSize {
					min: Some(8.0),
					max: Some(8.0),
				}),
				..RegisterFauxProviderOptions::default()
			}));
			faux.set_responses(vec![
				faux_assistant_message(
					faux_tool_call(
						"write",
						args.as_object().unwrap().clone(),
						Some(FauxToolCallOptions {
							id: Some("call_1".into()),
						}),
					),
					Some(FauxAssistantMessageOptions {
						stop_reason: Some(StopReason::ToolUse),
						..FauxAssistantMessageOptions::default()
					}),
				)
				.into(),
			]);
			let model = faux.get_model(None).unwrap();
			let events = faux.stream_simple(&model, &write_context(), Some(SimpleStreamOptions::default()));
			expect_growing_partial_arguments(events, &args).await;
		});
	}
}
