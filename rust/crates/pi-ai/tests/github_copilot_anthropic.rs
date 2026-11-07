//! Port of `packages/ai/test/github-copilot-anthropic.test.ts`.
//!
//! Differences from the TS:
//! - The TS mocks `@anthropic-ai/sdk` and records the client constructor options and the `beta.messages.create`
//!   params. Here the real client sends its request to the `TestHost` fetch, which answers with the same SSE body.
//!   Constructor assertions become assertions on that request: `apiKey: null` is a request without `x-api-key`,
//!   `authToken: "x"` is `authorization: Bearer x`, and `defaultHeaders.X` is the request header `X`. The `create`
//!   params are recorded through `onPayload`, which receives the params `create` is called with.

// The options' fields sit behind `Deref`, so the struct literal this lint suggests does not compile.
#![allow(clippy::field_reassign_with_default)]

mod anthropic_support;

use std::cell::RefCell;
use std::rc::Rc;

use anthropic_support::{SseEvent, event, model, sse_response};
use futures::StreamExt;
use pi_ai::api::anthropic_messages::{AnthropicOptions, stream};
use pi_ai::models::get_supported_thinking_levels;
use pi_ai::types::{AssistantMessageEvent, Model, OnPayload, TranscriptContext};
use pi_host::testing::{RecordedRequest, TestHost, run};
use serde_json::{Value, json};

/// The mocked SDK's `createSseResponse()`: no `message_stop`.
fn sse_events() -> Vec<SseEvent> {
	vec![
		event(
			"message_start",
			json!({
				"type": "message_start",
				"message": { "id": "msg_test", "usage": { "input_tokens": 10, "output_tokens": 0 } },
			}),
		),
		event(
			"message_delta",
			json!({ "type": "message_delta", "delta": { "stop_reason": "end_turn" }, "usage": { "output_tokens": 5 } }),
		),
	]
}

fn serve_sse(host: &TestHost) {
	host.on_fetch(|_| Box::pin(async { Ok(sse_response(&sse_events())) }));
}

fn context() -> TranscriptContext {
	anthropic_support::context(json!({
		"systemPrompt": "You are a helpful assistant.",
		"messages": [{ "role": "user", "content": "Hello", "timestamp": 1 }],
	}))
}

/// `mockState.createParams`: an `onPayload` that records the params and keeps them unchanged.
fn record_params() -> (OnPayload, Rc<RefCell<Option<Value>>>) {
	let params: Rc<RefCell<Option<Value>>> = Rc::default();
	let recorded = params.clone();
	let on_payload: OnPayload = Rc::new(move |payload, _| {
		*recorded.borrow_mut() = Some(payload);
		Box::pin(async { Ok(None) })
	});
	(on_payload, params)
}

/// `for await (const event of s) { if (event.type === "error") break; }`.
async fn drain(model: &Model, options: AnthropicOptions) {
	let mut events = Box::pin(stream(model, &context(), Some(options)).into_stream());
	while let Some(event) = events.next().await {
		if matches!(event, AssistantMessageEvent::Error { .. }) {
			break;
		}
	}
}

/// `params.betas ?? []` contains `beta`.
fn has_beta(params: &Value, beta: &str) -> bool {
	params
		.get("betas")
		.and_then(Value::as_array)
		.is_some_and(|betas| betas.contains(&json!(beta)))
}

fn levels(model: &Model) -> Vec<String> {
	get_supported_thinking_levels(model)
		.into_iter()
		.map(|level| level.as_str().to_string())
		.collect()
}

/// `expect(model.thinkingLevelMap).toMatchObject(expected)`.
fn assert_thinking_level_map_matches(model: &Model, expected: Value) {
	let map = serde_json::to_value(&model.thinking_level_map).unwrap();
	for (level, value) in expected.as_object().unwrap() {
		assert_eq!(map[level], *value, "thinkingLevelMap.{level}");
	}
}

fn messages_request(host: &TestHost) -> RecordedRequest {
	let requests = host.requests();
	assert_eq!(requests.len(), 1, "one request");
	requests[0].clone()
}

mod copilot_claude_via_anthropic_messages {
	use super::*;

	#[test]
	fn applies_copilot_specific_adaptive_thinking_effort_overrides() {
		let opus5 = model("github-copilot", "claude-opus-5");
		assert_eq!(json!(opus5.api), json!("anthropic-messages"));
		assert_eq!(opus5.context_window, 1000000.0);
		assert_thinking_level_map_matches(&opus5, json!({ "minimal": "low", "xhigh": "xhigh", "max": "max" }));
		assert!(levels(&opus5).contains(&"xhigh".to_string()));
		assert!(levels(&opus5).contains(&"max".to_string()));

		let opus55 = model("github-copilot", "claude-opus-5.5");
		assert_eq!(json!(opus55.api), json!("anthropic-messages"));
		assert_eq!(opus55.context_window, 1000000.0);
		assert_eq!(levels(&opus55), ["low", "medium", "high", "xhigh", "max"]);

		let sonnet46 = model("github-copilot", "claude-sonnet-4.6");
		assert_thinking_level_map_matches(&sonnet46, json!({ "minimal": "low", "max": "max" }));
		assert!(levels(&sonnet46).contains(&"max".to_string()));
		assert!(!levels(&sonnet46).contains(&"xhigh".to_string()));
	}

	#[test]
	fn uses_bearer_auth_copilot_headers_and_valid_anthropic_messages_payload() {
		run(|host| async move {
			serve_sse(&host);
			let model = model("github-copilot", "claude-sonnet-4.6");
			assert_eq!(json!(model.api), json!("anthropic-messages"));

			let (on_payload, params) = record_params();
			let mut options = AnthropicOptions::default();
			options.api_key = Some("tid_copilot_session_test_token".into());
			options.on_payload = Some(on_payload);
			drain(&model, options).await;

			let request = messages_request(&host);

			// Auth: apiKey null, authToken for Bearer
			assert_eq!(request.header("x-api-key"), None);
			assert_eq!(
				request.header("authorization"),
				Some("Bearer tid_copilot_session_test_token")
			);

			// Copilot static headers from model.headers
			assert!(request.header("user-agent").unwrap().contains("GitHubCopilotChat"));
			assert_eq!(request.header("copilot-integration-id"), Some("vscode-chat"));

			// Dynamic headers
			assert_eq!(request.header("x-initiator"), Some("user"));
			assert_eq!(request.header("openai-intent"), Some("conversation-edits"));

			// Payload is valid Anthropic Messages format
			let params = params.borrow().clone().expect("create was called");
			assert!(!has_beta(&params, "fine-grained-tool-streaming-2025-05-14"));
			assert_eq!(params["model"], "claude-sonnet-4.6");
			assert_eq!(params["stream"], true);
			assert_eq!(params["max_tokens"].as_f64(), Some(model.max_tokens));
			assert!(params["messages"].is_array());
		});
	}

	#[test]
	fn omits_interleaved_thinking_beta_for_adaptive_thinking_models() {
		run(|host| async move {
			serve_sse(&host);
			let model = model("github-copilot", "claude-sonnet-4.6");
			let (on_payload, params) = record_params();
			let mut options = AnthropicOptions {
				interleaved_thinking: Some(true),
				..AnthropicOptions::default()
			};
			options.api_key = Some("tid_copilot_session_test_token".into());
			options.on_payload = Some(on_payload);
			drain(&model, options).await;

			let params = params.borrow().clone().expect("create was called");
			assert!(!has_beta(&params, "interleaved-thinking-2025-05-14"));
		});
	}
}
