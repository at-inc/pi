//! Port of `packages/agent/test/proxy.test.ts`.
//!
//! `vi.stubGlobal("fetch", ...)` becomes a `TestHost` fetch handler. The timing guard of the #9265 test measures wall
//! time with `std::time::Instant`: the test host's `performance.now()` is a virtual clock that only moves on sleeps.

use indexmap::IndexMap;
use pi_agent::proxy::{ProxyStreamOptions, stream_proxy};
use pi_ai::types::{AssistantContent, AssistantMessageEvent, Context, Model, StopReason};
use pi_ai::utils::transcript::normalize_context;
use pi_host::HttpResponse;
use pi_host::testing::{TestHost, run};
use serde_json::{Value, json};

fn model() -> Model {
	serde_json::from_value(json!({
		"id": "gpt-5.4",
		"name": "GPT-5.4",
		"api": "openai-responses",
		"provider": "openai",
		"baseUrl": "https://api.openai.com/v1",
		"reasoning": true,
		"input": ["text"],
		"cost": { "input": 0, "output": 0, "cacheRead": 0, "cacheWrite": 0 },
		"contextWindow": 400000,
		"maxTokens": 128000,
	}))
	.unwrap()
}

fn usage() -> Value {
	json!({
		"input": 0,
		"output": 0,
		"cacheRead": 0,
		"cacheWrite": 0,
		"totalTokens": 0,
		"cost": { "input": 0, "output": 0, "cacheRead": 0, "cacheWrite": 0, "total": 0 },
	})
}

fn stub_fetch(host: &TestHost, body: String) {
	host.on_fetch(move |_request| {
		let body = body.clone();
		Box::pin(async move { Ok(HttpResponse::from_bytes(200, IndexMap::new(), body)) })
	});
}

fn sse_body(events: &[Value]) -> String {
	events
		.iter()
		.map(|event| format!("data: {}\n\n", pi_host::js::json_stringify(event)))
		.collect()
}

fn options() -> ProxyStreamOptions {
	ProxyStreamOptions {
		auth_token: "test-token".into(),
		proxy_url: "https://proxy.example.com".into(),
		..ProxyStreamOptions::default()
	}
}

fn empty_context() -> pi_ai::types::TranscriptContext {
	normalize_context(&Context {
		system_prompt: Some(String::new()),
		..Context::default()
	})
}

fn event_type(event: &AssistantMessageEvent) -> String {
	serde_json::to_value(event).unwrap()["type"]
		.as_str()
		.unwrap()
		.to_string()
}

mod stream_proxy {
	use super::*;

	#[test]
	fn preserves_tool_call_metadata_received_only_on_toolcall_end() {
		run(|host| async move {
			let proxy_events = [
				json!({ "type": "start" }),
				json!({ "type": "toolcall_start", "contentIndex": 0, "id": "call_test|fc_test", "toolName": "lookup" }),
				json!({ "type": "toolcall_delta", "contentIndex": 0, "delta": "{\"value\":\"hello\"}" }),
				json!({
					"type": "toolcall_end",
					"contentIndex": 0,
					"toolCall": {
						"type": "toolCall",
						"id": "call_test|fc_test",
						"name": "lookup",
						"arguments": { "value": "hello" },
						"namespace": "dynamic_tools",
					},
				}),
				json!({ "type": "done", "reason": "toolUse", "usage": usage() }),
			];
			stub_fetch(&host, sse_body(&proxy_events));

			let stream = stream_proxy(&model(), &empty_context(), options());
			let mut events = Vec::new();
			while let Some(event) = stream.next().await {
				events.push(event);
			}
			let result = stream.result().await.unwrap();
			let end_event = events
				.iter()
				.find(|event| matches!(event, AssistantMessageEvent::ToolcallEnd { .. }))
				.expect("toolcall_end event");

			let AssistantMessageEvent::ToolcallEnd { tool_call, .. } = end_event else {
				unreachable!();
			};
			assert_eq!(tool_call.namespace.as_deref(), Some("dynamic_tools"));
			let AssistantContent::ToolCall(block) = &result.content[0] else {
				panic!("expected a tool call, got {:?}", result.content[0]);
			};
			assert_eq!(Value::Object(block.arguments.clone()), json!({ "value": "hello" }));
			assert_eq!(block.namespace.as_deref(), Some("dynamic_tools"));
		});
	}

	// Regression tests for https://github.com/earendil-works/pi/issues/8996
	#[test]
	fn processes_terminal_metadata_when_the_event_is_not_newline_terminated() {
		run(|host| async move {
			let start = format!("data: {}\n\n", pi_host::js::json_stringify(&json!({ "type": "start" })));
			let done = format!(
				"data: {}",
				pi_host::js::json_stringify(
					&json!({ "type": "done", "reason": "stop", "usage": usage(), "providerThinkingLevel": "high" })
				)
			);
			stub_fetch(&host, start + &done);

			let stream = stream_proxy(&model(), &empty_context(), options());
			let mut events = Vec::new();
			while let Some(event) = stream.next().await {
				events.push(event);
			}
			let result = stream.result().await.unwrap();

			assert_eq!(events.iter().map(event_type).collect::<Vec<_>>(), ["start", "done"]);
			assert_eq!(result.stop_reason, StopReason::Stop);
			assert_eq!(result.provider_thinking_level.as_deref(), Some("high"));
		});
	}

	#[test]
	fn emits_an_error_instead_of_hanging_when_the_stream_ends_without_a_terminal_event() {
		run(|host| async move {
			let body = format!("data: {}\n\n", pi_host::js::json_stringify(&json!({ "type": "start" })));
			stub_fetch(&host, body);

			let stream = stream_proxy(&model(), &empty_context(), options());
			let mut events = Vec::new();
			while let Some(event) = stream.next().await {
				events.push(event);
			}
			let result = stream.result().await.unwrap();

			assert_eq!(events.iter().map(event_type).collect::<Vec<_>>(), ["start", "error"]);
			assert_eq!(result.stop_reason, StopReason::Error);
			assert!(
				result
					.error_message
					.as_deref()
					.unwrap()
					.contains("Connection closed by proxy server")
			);
		});
	}

	// #9265
	#[test]
	fn shows_growing_partial_arguments_without_keeping_the_raw_text_on_the_block_in_linear_time() {
		run(|host| async move {
			let content = "line \"quoted\" \\ end\n".repeat(20_000);
			let text = pi_host::js::json_stringify(&json!({ "path": "a.ts", "content": content }));
			let mut proxy_events = vec![
				json!({ "type": "start" }),
				json!({ "type": "toolcall_start", "contentIndex": 0, "id": "call_1", "toolName": "write" }),
			];
			let mut index = 0;
			while index < text.len() {
				let end = (index + 32).min(text.len());
				proxy_events.push(json!({ "type": "toolcall_delta", "contentIndex": 0, "delta": &text[index..end] }));
				index += 32;
			}
			proxy_events.push(json!({
				"type": "toolcall_end",
				"contentIndex": 0,
				"toolCall": { "type": "toolCall", "id": "call_1", "name": "write", "arguments": { "path": "a.ts", "content": content } },
			}));
			proxy_events.push(json!({ "type": "done", "reason": "toolUse", "usage": usage() }));
			stub_fetch(&host, sse_body(&proxy_events));

			let started = std::time::Instant::now();
			let stream = stream_proxy(&model(), &empty_context(), options());
			let mut shown = 0;
			let mut deltas = 0;
			while let Some(event) = stream.next().await {
				let AssistantMessageEvent::ToolcallDelta { partial, .. } = &event else {
					continue;
				};
				let block = serde_json::to_value(&partial.borrow().content[0]).unwrap();
				let mut keys: Vec<&String> = block.as_object().unwrap().keys().collect();
				keys.sort();
				assert_eq!(keys, ["arguments", "id", "name", "type"]);
				let Some(partial) = block["arguments"]["content"].as_str() else {
					continue;
				};
				// Comparing the whole text on every delta would make this test itself quadratic.
				if deltas % 500 == 0 {
					assert!(content.starts_with(partial));
				}
				deltas += 1;
				assert!(partial.len() >= shown);
				shown = partial.len();
			}
			let result = stream.result().await.unwrap();

			assert!(shown > content.len() - 64);
			assert_eq!(
				serde_json::to_value(&result.content).unwrap(),
				json!([{ "type": "toolCall", "id": "call_1", "name": "write", "arguments": { "path": "a.ts", "content": content } }])
			);
			// Re-parsing the accumulated 520 KB of arguments on each of its 16,000 deltas took minutes.
			assert!(started.elapsed().as_millis() < 10_000);
		});
	}
}
