//! Port of `packages/ai/test/openai-codex-stream.test.ts`.
//!
//! Differences from the TS:
//! - `process.env.PI_CODING_AGENT_DIR` temp dirs are not set: the Codex adapter reads no agent dir. The fetch mocks
//!   keep their GitHub and raw.githubusercontent routes so the request routing matches.
//! - Fetch mocks that assert inside the handler record the request instead, and the test asserts on it after the
//!   stream ends. `decodeCodexRequestBody` decodes by the `content-encoding` header: zstd bodies are only sent with the
//!   `native` feature, so "zstd-compresses SSE request bodies" runs only with it.
//! - `vi.useFakeTimers()` and `advanceTimersByTimeAsync` are the test host's virtual timers, which fire whenever every
//!   task waits. `await vi.advanceTimersByTimeAsync(0)` followed by an assertion is a zero-delay sleep joined with the
//!   stream. `vi.setSystemTime` moves the virtual clock with `host.advance`. A `setTimeout` spy that checks a retry delay
//!   becomes the virtual time between two Codex requests.
//! - The `MockWebSocket` classes are `pi_host::testing::FakeWebSocket`s from `host.on_websocket`; `queueMicrotask`
//!   dispatches are `emit_later`. A mock with a `readyState` sets one; `close()` that sets `readyState = 3` is an
//!   `on_close` handler.
//! - `afterEach` (closing cached sessions, resetting debug stats) runs at the start and end of every test case.
//! - `it.each` cases run one after another in one `#[test]`, each case in its own test host.
//! - `ReadableStream` cancellation is the response body being dropped before it finished.

use std::cell::{Cell, RefCell};
use std::future::Future;
use std::rc::Rc;

use base64::Engine;
use bytes::Bytes;
use futures::StreamExt;
use indexmap::IndexMap;
use pi_ai::api::openai_codex_responses::{
	OpenAICodexResponsesOptions, close_openai_codex_websocket_sessions, get_openai_codex_websocket_debug_stats,
	reset_openai_codex_websocket_debug_stats, stream as stream_openai_codex_responses,
	stream_simple as stream_simple_openai_codex_responses,
};
use pi_ai::providers::model_schema::{CacheRetention, ThinkingLevel};
use pi_ai::types::{
	AssistantContent, AssistantMessage, AssistantMessageEvent, Context, Message, Model, OnPayload,
	OnProviderStreamEvent, ProviderRequestOptions, SimpleStreamOptions, StopReason, StreamOptions, TranscriptContext,
	Transport,
};
use pi_ai::utils::event_stream::AssistantMessageEventStream;
use pi_ai::utils::identity::format_user_agent;
use pi_ai::utils::transcript::normalize_context;
use pi_host::js::json_stringify;
use pi_host::testing::{FakeWebSocket, TestHost};
use pi_host::websocket::{OPEN, WebSocketData, WebSocketEvent};
use pi_host::{AbortController, HttpRequest, HttpResponse, LocalBoxFuture, PiError, PiResult};
use serde_json::{Value, json};

/// The test body with `afterEach` around it: cached sessions closed and debug stats reset.
fn run_codex<F, Fut, T>(test: F) -> T
where
	F: FnOnce(Rc<TestHost>) -> Fut,
	Fut: Future<Output = T>,
{
	pi_host::testing::run(move |host| {
		close_openai_codex_websocket_sessions(None);
		reset_openai_codex_websocket_debug_stats(None);
		let test = test(host);
		async move {
			let output = test.await;
			close_openai_codex_websocket_sessions(None);
			reset_openai_codex_websocket_debug_stats(None);
			output
		}
	})
}

fn mock_token(account_id: &str) -> String {
	let payload = base64::engine::general_purpose::STANDARD.encode(json_stringify(
		&json!({ "https://api.openai.com/auth": { "chatgpt_account_id": account_id } }),
	));
	format!("aaa.{payload}.bbb")
}

#[cfg(feature = "native")]
fn zstd_decompress(body: &[u8]) -> Vec<u8> {
	zstd::decode_all(body).expect("zstd body decodes")
}

#[cfg(not(feature = "native"))]
fn zstd_decompress(_body: &[u8]) -> Vec<u8> {
	panic!("zstd request bodies are only sent with the native feature")
}

fn decode_codex_request_body(request: &HttpRequest) -> Option<Value> {
	let body = request.body.as_deref()?;
	if request.header("content-encoding") == Some("zstd") {
		return Some(serde_json::from_slice(&zstd_decompress(body)).expect("request body is JSON"));
	}
	Some(serde_json::from_slice(body).expect("request body is JSON"))
}

fn sse_data(event: Value) -> String {
	format!("data: {}", json_stringify(&event))
}

fn build_sse_payload(status: &str, include_done: bool, end_turn: Option<bool>) -> String {
	let terminal_type = if status == "incomplete" {
		"response.incomplete"
	} else {
		"response.completed"
	};
	let mut response = serde_json::Map::new();
	response.insert("status".into(), json!(status));
	if let Some(end_turn) = end_turn {
		response.insert("end_turn".into(), json!(end_turn));
	}
	response.insert(
		"incomplete_details".into(),
		if status == "incomplete" {
			json!({ "reason": "max_output_tokens" })
		} else {
			Value::Null
		},
	);
	response.insert(
		"usage".into(),
		json!({
			"input_tokens": 5,
			"output_tokens": 3,
			"total_tokens": 8,
			"input_tokens_details": { "cached_tokens": 0 },
		}),
	);
	let mut events = vec![
		sse_data(json!({
			"type": "response.output_item.added",
			"item": { "type": "message", "id": "msg_1", "role": "assistant", "status": "in_progress", "content": [] },
		})),
		sse_data(json!({ "type": "response.content_part.added", "part": { "type": "output_text", "text": "" } })),
		sse_data(json!({ "type": "response.output_text.delta", "delta": "Hello" })),
		sse_data(json!({
			"type": "response.output_item.done",
			"item": {
				"type": "message",
				"id": "msg_1",
				"role": "assistant",
				"status": "completed",
				"content": [{ "type": "output_text", "text": "Hello" }],
			},
		})),
		sse_data(json!({ "type": terminal_type, "response": Value::Object(response) })),
	];
	if include_done {
		events.push("data: [DONE]".into());
	}
	format!("{}\n\n", events.join("\n\n"))
}

/// The SSE payload the older cases spell out inline: a "Hello" message and a `response.completed` with only
/// `status`, an optional `service_tier` and `usage`.
fn inline_sse_payload(service_tier: Option<&str>, usage: Value) -> String {
	let mut response = serde_json::Map::new();
	response.insert("status".into(), json!("completed"));
	if let Some(service_tier) = service_tier {
		response.insert("service_tier".into(), json!(service_tier));
	}
	response.insert("usage".into(), usage);
	let events = [
		sse_data(json!({
			"type": "response.output_item.added",
			"item": { "type": "message", "id": "msg_1", "role": "assistant", "status": "in_progress", "content": [] },
		})),
		sse_data(json!({ "type": "response.content_part.added", "part": { "type": "output_text", "text": "" } })),
		sse_data(json!({ "type": "response.output_text.delta", "delta": "Hello" })),
		sse_data(json!({
			"type": "response.output_item.done",
			"item": {
				"type": "message",
				"id": "msg_1",
				"role": "assistant",
				"status": "completed",
				"content": [{ "type": "output_text", "text": "Hello" }],
			},
		})),
		sse_data(json!({ "type": "response.completed", "response": Value::Object(response) })),
	];
	format!("{}\n\n", events.join("\n\n"))
}

fn small_usage() -> Value {
	json!({
		"input_tokens": 5,
		"output_tokens": 3,
		"total_tokens": 8,
		"input_tokens_details": { "cached_tokens": 0 },
	})
}

fn headers(entries: &[(&str, &str)]) -> IndexMap<String, String> {
	entries
		.iter()
		.map(|(name, value)| (name.to_string(), value.to_string()))
		.collect()
}

fn sse_response(sse: String) -> HttpResponse {
	HttpResponse::from_bytes(200, headers(&[("content-type", "text/event-stream")]), sse)
}

fn text_response(status: u16, response_headers: &[(&str, &str)], body: String) -> HttpResponse {
	HttpResponse::from_bytes(status, headers(response_headers), body)
}

/// `fetchMock` of the cases that route by URL: GitHub release and prompt URLs, the Codex endpoint, else 404.
fn routed_fetch(
	host: &TestHost,
	captured: Rc<RefCell<Vec<HttpRequest>>>,
	codex_response: impl Fn() -> HttpResponse + 'static,
) {
	host.on_fetch(move |request| {
		let url = request.url.clone();
		let response = if url == "https://api.github.com/repos/openai/codex/releases/latest" {
			text_response(200, &[], json_stringify(&json!({ "tag_name": "rust-v0.0.0" })))
		} else if url.starts_with("https://raw.githubusercontent.com/openai/codex/") {
			text_response(200, &[("etag", "\"etag\"")], "PROMPT".into())
		} else if url == "https://chatgpt.com/backend-api/codex/responses" {
			captured.borrow_mut().push(request);
			codex_response()
		} else {
			text_response(404, &[], "not found".into())
		};
		Box::pin(async move { Ok(response) })
	});
}

/// Serves every request with `response` and records it.
fn capturing_fetch(
	host: &TestHost,
	captured: Rc<RefCell<Vec<HttpRequest>>>,
	response: impl Fn() -> HttpResponse + 'static,
) {
	host.on_fetch(move |request| {
		captured.borrow_mut().push(request);
		let response = response();
		Box::pin(async move { Ok(response) })
	});
}

fn codex_model(id: &str, name: &str) -> Model {
	serde_json::from_value(json!({
		"id": id,
		"name": name,
		"api": "openai-codex-responses",
		"provider": "openai-codex",
		"baseUrl": "https://chatgpt.com/backend-api",
		"reasoning": true,
		"input": ["text"],
		"cost": { "input": 0, "output": 0, "cacheRead": 0, "cacheWrite": 0 },
		"contextWindow": 400000,
		"maxTokens": 128000,
	}))
	.expect("valid model")
}

fn gpt_51_codex() -> Model {
	codex_model("gpt-5.1-codex", "GPT-5.1 Codex")
}

fn user_message(text: &str, timestamp: f64) -> Message {
	serde_json::from_value(json!({ "role": "user", "content": text, "timestamp": timestamp })).expect("valid message")
}

fn context(system_prompt: Option<&str>, messages: Vec<Message>) -> TranscriptContext {
	normalize_context(&Context {
		system_prompt: system_prompt.map(str::to_string),
		messages,
		tools: None,
	})
}

fn say_hello_context(timestamp: f64) -> TranscriptContext {
	context(
		Some("You are a helpful assistant."),
		vec![user_message("Say hello", timestamp)],
	)
}

fn options(
	api_key: &str,
	configure: impl FnOnce(&mut OpenAICodexResponsesOptions),
) -> Option<OpenAICodexResponsesOptions> {
	let mut options = OpenAICodexResponsesOptions {
		base: StreamOptions {
			base: ProviderRequestOptions {
				api_key: Some(api_key.into()),
				..ProviderRequestOptions::default()
			},
			..StreamOptions::default()
		},
		..OpenAICodexResponsesOptions::default()
	};
	configure(&mut options);
	Some(options)
}

fn sse_options(api_key: &str) -> Option<OpenAICodexResponsesOptions> {
	options(api_key, |options| options.transport = Some(Transport::Sse))
}

fn text_of(message: &AssistantMessage) -> Option<String> {
	message.content.iter().find_map(|content| match content {
		AssistantContent::Text(text) => Some(text.text.clone()),
		_ => None,
	})
}

async fn result(stream: AssistantMessageEventStream) -> AssistantMessage {
	stream.result().await.expect("stream has a result")
}

/// `onProviderStreamEvent` pushing into `providerEvents` and `providerEventModels`.
type RecordedProviderEvents = (OnProviderStreamEvent, Rc<RefCell<Vec<Value>>>, Rc<RefCell<Vec<Model>>>);

fn provider_event_recorder() -> RecordedProviderEvents {
	let events = Rc::new(RefCell::new(Vec::new()));
	let models = Rc::new(RefCell::new(Vec::new()));
	let (recorded_events, recorded_models) = (events.clone(), models.clone());
	let callback: OnProviderStreamEvent = Rc::new(
		move |event: &Value, model: &Model| -> LocalBoxFuture<'static, PiResult<()>> {
			recorded_events.borrow_mut().push(event.clone());
			recorded_models.borrow_mut().push(model.clone());
			Box::pin(async { Ok(()) })
		},
	);
	(callback, events, models)
}

/// `onPayload` storing the payload and keeping it.
fn payload_recorder() -> (OnPayload, Rc<RefCell<Option<Value>>>) {
	let payload = Rc::new(RefCell::new(None));
	let recorded = payload.clone();
	let callback: OnPayload = Rc::new(
		move |payload: Value, _model: &Model| -> LocalBoxFuture<'static, PiResult<Option<Value>>> {
			*recorded.borrow_mut() = Some(payload);
			Box::pin(async { Ok(None) })
		},
	);
	(callback, payload)
}

fn event_types(events: &[Value]) -> Vec<String> {
	events
		.iter()
		.map(|event| event["type"].as_str().unwrap_or_default().to_string())
		.collect()
}

/// `event.type` of an assistant message event.
fn event_name(event: &AssistantMessageEvent) -> &'static str {
	match event {
		AssistantMessageEvent::Start { .. } => "start",
		AssistantMessageEvent::TextStart { .. } => "text_start",
		AssistantMessageEvent::TextDelta { .. } => "text_delta",
		AssistantMessageEvent::TextEnd { .. } => "text_end",
		AssistantMessageEvent::ThinkingStart { .. } => "thinking_start",
		AssistantMessageEvent::ThinkingDelta { .. } => "thinking_delta",
		AssistantMessageEvent::ThinkingEnd { .. } => "thinking_end",
		AssistantMessageEvent::ToolcallStart { .. } => "toolcall_start",
		AssistantMessageEvent::ToolcallDelta { .. } => "toolcall_delta",
		AssistantMessageEvent::ToolcallEnd { .. } => "toolcall_end",
		AssistantMessageEvent::Done { .. } => "done",
		AssistantMessageEvent::Error { .. } => "error",
	}
}

fn message_event(event: &Value) -> WebSocketEvent {
	WebSocketEvent::Message {
		data: WebSocketData::Text(json_stringify(event)),
	}
}

fn message_events(events: &[Value]) -> Vec<WebSocketEvent> {
	events.iter().map(message_event).collect()
}

/// A socket that dispatches `open` from a microtask, like the mocks' constructors.
fn opening_socket(url: &str, headers: &IndexMap<String, String>) -> Rc<FakeWebSocket> {
	let socket = FakeWebSocket::new(url, headers);
	socket.emit_later(vec![WebSocketEvent::Open]);
	socket
}

/// The `MockWebSocket` with `readyState`: opens from a microtask and `close()` sets `readyState = 3`.
fn ready_socket(url: &str, headers: &IndexMap<String, String>) -> Rc<FakeWebSocket> {
	let socket = opening_socket(url, headers);
	socket.set_ready_state(Some(OPEN));
	socket.on_close(|socket, _code, _reason| socket.set_ready_state(Some(3)));
	socket
}

/// `expect(actual).toMatchObject(expected)` for JSON values; numbers compare as `f64`, like JS numbers.
fn assert_match_object(actual: &Value, expected: &Value) {
	match (actual, expected) {
		(Value::Object(actual), Value::Object(expected)) => {
			for (key, expected) in expected {
				assert_match_object(actual.get(key).unwrap_or_else(|| panic!("missing key {key}")), expected);
			}
		}
		(Value::Array(actual), Value::Array(expected)) => {
			assert_eq!(actual.len(), expected.len());
			for (actual, expected) in actual.iter().zip(expected) {
				assert_match_object(actual, expected);
			}
		}
		(Value::Number(actual), Value::Number(expected)) => assert_eq!(actual.as_f64(), expected.as_f64()),
		_ => assert_eq!(actual, expected),
	}
}

/// `Promise.race([result, timeout])` with a `setTimeout` rejection of `message` after `ms`.
async fn race_timeout(stream: AssistantMessageEventStream, ms: f64, message: &str) -> AssistantMessage {
	let timeout = pi_host::sleep(ms, None);
	futures::pin_mut!(timeout);
	let result = stream.result();
	futures::pin_mut!(result);
	match futures::future::select(result, timeout).await {
		futures::future::Either::Left((result, _)) => result.expect("stream has a result"),
		futures::future::Either::Right(_) => panic!("{message}"),
	}
}

/// A response whose body sends `sse` and then stays open.
fn open_sse_response(sse: String) -> HttpResponse {
	let mut response = sse_response(String::new());
	response.body = futures::stream::once(async move { Ok(Bytes::from(sse)) })
		.chain(futures::stream::pending())
		.boxed_local();
	response
}

/// Sets `cancelled` when the body is dropped before it closed, like `ReadableStream`'s `cancel()`.
struct CancelOnDrop {
	cancelled: Rc<Cell<bool>>,
	closed: Rc<Cell<bool>>,
}

impl Drop for CancelOnDrop {
	fn drop(&mut self) {
		if !self.closed.get() {
			self.cancelled.set(true);
		}
	}
}

/// `fetchMock` of the fallback cases: the Codex endpoint serves `buildSSEPayload`, any other URL throws.
fn codex_only_fetch(host: &TestHost, calls: Rc<Cell<usize>>) {
	host.on_fetch(move |request| {
		calls.set(calls.get() + 1);
		Box::pin(async move {
			if request.url != "https://chatgpt.com/backend-api/codex/responses" {
				return Err(PiError::new(format!("Unexpected URL: {}", request.url)));
			}
			Ok(sse_response(build_sse_payload("completed", false, None)))
		})
	});
}

/// `vi.setSystemTime(epochMs)` on the virtual clock, which only moves forward.
fn set_system_time(host: &TestHost, epoch_ms: f64) {
	let now = pi_host::now_ms();
	assert!(epoch_ms >= now, "the virtual clock only moves forward");
	host.advance(epoch_ms - now);
}

/// `fetchMock` of the retry cases: the first `failures` Codex requests answer `failure()`, the rest the SSE
/// payload. Records the virtual time of each Codex request.
fn retrying_fetch(
	host: &TestHost,
	failures: usize,
	failure: impl Fn() -> HttpResponse + 'static,
) -> Rc<RefCell<Vec<f64>>> {
	let request_times: Rc<RefCell<Vec<f64>>> = Rc::default();
	let times = request_times.clone();
	host.on_fetch(move |request| {
		if request.url != "https://chatgpt.com/backend-api/codex/responses" {
			let url = request.url.clone();
			return Box::pin(async move { Err(PiError::new(format!("Unexpected URL: {url}"))) });
		}
		times.borrow_mut().push(pi_host::monotonic_ms());
		let response = if times.borrow().len() <= failures {
			failure()
		} else {
			sse_response(build_sse_payload("completed", false, None))
		};
		Box::pin(async move { Ok(response) })
	});
	request_times
}

fn delays(request_times: &[f64]) -> Vec<f64> {
	request_times.windows(2).map(|pair| pair[1] - pair[0]).collect()
}

fn rate_limited(retry_headers: &[(&str, &str)]) -> HttpResponse {
	text_response(
		429,
		retry_headers,
		json_stringify(&json!({ "error": { "code": "rate_limit_exceeded", "message": "rate limited" } })),
	)
}

mod openai_codex_streaming {
	use super::*;

	#[test]
	fn streams_sse_responses_and_forwards_raw_provider_events() {
		run_codex(|host| async move {
			let token = mock_token("acc_test");
			let captured = Rc::new(RefCell::new(Vec::new()));
			routed_fetch(&host, captured.clone(), || {
				sse_response(inline_sse_payload(None, small_usage()))
			});

			let model = gpt_51_codex();
			let context = say_hello_context(pi_host::now_ms());
			let (on_provider_stream_event, provider_events, provider_event_models) = provider_event_recorder();
			let stream_result = stream_openai_codex_responses(
				&model,
				&context,
				options(&token, |options| {
					options.transport = Some(Transport::Sse);
					options.on_provider_stream_event = Some(on_provider_stream_event);
				}),
			);
			let mut saw_text_delta = false;
			let mut saw_done = false;

			while let Some(event) = stream_result.next().await {
				if let AssistantMessageEvent::TextDelta { .. } = event {
					saw_text_delta = true;
				}
				if let AssistantMessageEvent::Done { message, .. } = &event {
					saw_done = true;
					assert_eq!(text_of(&message.borrow()).as_deref(), Some("Hello"));
				}
			}

			let captured = captured.borrow();
			assert_eq!(captured.len(), 1);
			let request = &captured[0];
			assert_eq!(
				request.header("Authorization"),
				Some(format!("Bearer {token}").as_str())
			);
			assert_eq!(request.header("chatgpt-account-id"), Some("acc_test"));
			assert_eq!(request.header("OpenAI-Beta"), Some("responses=experimental"));
			assert_eq!(request.header("originator"), Some("pi"));
			assert_eq!(request.header("User-Agent"), Some(format_user_agent(None).as_str()));
			assert_eq!(request.header("accept"), Some("text/event-stream"));
			assert!(request.header("x-api-key").is_none());

			assert!(saw_text_delta);
			assert!(saw_done);
			assert_eq!(
				event_types(&provider_events.borrow()),
				[
					"response.output_item.added",
					"response.content_part.added",
					"response.output_text.delta",
					"response.output_item.done",
					"response.completed",
				]
			);
			assert_eq!(*provider_event_models.borrow(), vec![model.clone(); 5]);
		});
	}

	// Regression test for https://github.com/earendil-works/pi/issues/9047
	#[test]
	fn processes_a_terminal_sse_event_without_a_trailing_blank_line() {
		run_codex(|_host| async move {
			let token = mock_token("acc_test");
			let sse = build_sse_payload("completed", false, None).trim_end().to_string();
			let model = gpt_51_codex();
			let context = say_hello_context(pi_host::now_ms());
			let result_stream = stream_openai_codex_responses(
				&model,
				&context,
				options(&token, |options| {
					options.transport = Some(Transport::Sse);
					options.fetch = Some(Rc::new(move |_| {
						let sse = sse.clone();
						Box::pin(async move { Ok(sse_response(sse)) })
					}));
				}),
			);
			let result = result(result_stream).await;

			assert_eq!(result.stop_reason, StopReason::Stop);
			assert_eq!(text_of(&result).as_deref(), Some("Hello"));
		});
	}

	#[test]
	fn completes_after_response_completed_even_when_the_sse_body_stays_open() {
		run_codex(|host| async move {
			let token = mock_token("acc_test");
			let sse = build_sse_payload("completed", true, Some(false));
			routed_fetch(&host, Rc::default(), move || open_sse_response(sse.clone()));

			let model = gpt_51_codex();
			let context = say_hello_context(pi_host::now_ms());

			let result = race_timeout(
				stream_openai_codex_responses(&model, &context, sse_options(&token)),
				1000.0,
				"Timed out waiting for completed SSE stream",
			)
			.await;

			assert_eq!(text_of(&result).as_deref(), Some("Hello"));
			assert_eq!(result.stop_reason, StopReason::Stop);
			assert_eq!(result.end_turn, Some(false));
		});
	}

	#[test]
	fn maps_response_incomplete_to_stop_reason_length_even_when_the_sse_body_stays_open() {
		run_codex(|host| async move {
			let token = mock_token("acc_test");
			let sse = build_sse_payload("incomplete", false, None);
			routed_fetch(&host, Rc::default(), move || open_sse_response(sse.clone()));

			let model = gpt_51_codex();
			let context = say_hello_context(pi_host::now_ms());

			let result = race_timeout(
				stream_openai_codex_responses(&model, &context, sse_options(&token)),
				1000.0,
				"Timed out waiting for incomplete SSE stream",
			)
			.await;

			assert_eq!(text_of(&result).as_deref(), Some("Hello"));
			assert_eq!(result.stop_reason, StopReason::Length);
		});
	}

	#[test]
	fn aborts_sse_fetch_after_the_configured_http_timeout_when_response_headers_do_not_arrive() {
		run_codex(|host| async move {
			let token = mock_token("acc_test");
			let calls = Rc::new(Cell::new(0));
			let counted = calls.clone();
			host.on_fetch(move |request| {
				counted.set(counted.get() + 1);
				Box::pin(async move {
					if request.url != "https://chatgpt.com/backend-api/codex/responses" {
						return Err(PiError::new(format!("Unexpected URL: {}", request.url)));
					}
					let Some(signal) = request.signal else {
						return Err(PiError::new("Expected SSE fetch to receive an abort signal"));
					};
					Err(signal.aborted_future().await)
				})
			});

			let model = gpt_51_codex();
			let context = say_hello_context(pi_host::now_ms());

			let result = result(stream_openai_codex_responses(
				&model,
				&context,
				options(&token, |options| {
					options.transport = Some(Transport::Sse);
					options.timeout_ms = Some(10.0);
				}),
			))
			.await;

			assert_eq!(calls.get(), 1);
			assert_eq!(result.stop_reason, StopReason::Error);
			assert_eq!(
				result.error_message.as_deref(),
				Some("Codex SSE response headers timed out after 10ms")
			);
		});
	}

	#[test]
	fn aborts_sse_body_reads_after_response_headers_arrive() {
		run_codex(|host| async move {
			let token = mock_token("acc_test");
			let cancelled = Rc::new(Cell::new(false));
			let first_chunk = format!(
				"{}\n\n",
				[
					sse_data(json!({
						"type": "response.output_item.added",
						"item": { "type": "message", "id": "msg_1", "role": "assistant", "status": "in_progress", "content": [] },
					})),
					sse_data(
						json!({ "type": "response.content_part.added", "part": { "type": "output_text", "text": "" } })
					),
					sse_data(json!({ "type": "response.output_text.delta", "delta": "one" })),
				]
				.join("\n\n")
			);
			let second_chunk = format!(
				"{}\n\n",
				sse_data(json!({ "type": "response.output_text.delta", "delta": "two" }))
			);
			let last_chunk = format!(
				"{}\n\n",
				[
					sse_data(json!({
						"type": "response.output_item.done",
						"item": {
							"type": "message",
							"id": "msg_1",
							"role": "assistant",
							"status": "completed",
							"content": [{ "type": "output_text", "text": "onetwo" }],
						},
					})),
					sse_data(json!({
						"type": "response.completed",
						"response": { "status": "completed", "usage": small_usage() },
					})),
				]
				.join("\n\n")
			);
			let body_cancelled = cancelled.clone();
			host.on_fetch(move |_| {
				let closed = Rc::new(Cell::new(false));
				let guard = CancelOnDrop {
					cancelled: body_cancelled.clone(),
					closed: closed.clone(),
				};
				let chunks = [first_chunk.clone(), second_chunk.clone(), last_chunk.clone()];
				let body = futures::stream::unfold(0, move |step| {
					let (chunks, closed) = (chunks.clone(), closed.clone());
					async move {
						match step {
							0 => Some((Ok(Bytes::from(chunks[0].clone())), 1)),
							1 => {
								pi_host::sleep(10.0, None).await.ok();
								Some((Ok(Bytes::from(chunks[1].clone())), 2))
							}
							2 => {
								pi_host::sleep(10.0, None).await.ok();
								Some((Ok(Bytes::from(chunks[2].clone())), 3))
							}
							_ => {
								closed.set(true);
								None
							}
						}
					}
				})
				.map(move |chunk| {
					let _ = &guard;
					chunk
				})
				.boxed_local();
				let mut response = sse_response(String::new());
				response.body = body;
				Box::pin(async move { Ok(response) })
			});

			let model = gpt_51_codex();
			let context = say_hello_context(pi_host::now_ms());
			let controller = AbortController::new();
			let mut events: Vec<String> = Vec::new();

			let result_stream = stream_openai_codex_responses(
				&model,
				&context,
				options(&token, |options| {
					options.transport = Some(Transport::Sse);
					options.signal = Some(controller.signal());
				}),
			);
			while let Some(event) = result_stream.next().await {
				let label = match &event {
					AssistantMessageEvent::TextDelta { delta, .. } => format!("text_delta:{delta}"),
					other => event_name(other).to_string(),
				};
				let abort = label == "text_delta:one";
				events.push(label);
				if abort {
					controller.abort();
				}
			}

			let result = result(result_stream).await;
			assert_eq!(result.stop_reason, StopReason::Aborted);
			assert_eq!(result.error_message.as_deref(), Some("Request was aborted"));
			assert!(events.contains(&"text_delta:one".to_string()));
			assert!(!events.contains(&"text_delta:two".to_string()));
			assert!(cancelled.get());
		});
	}

	#[test]
	fn sets_session_id_x_client_request_id_headers_and_prompt_cache_key_when_session_id_is_provided() {
		run_codex(|host| async move {
			let token = mock_token("acc_test");
			let captured = Rc::new(RefCell::new(Vec::new()));
			routed_fetch(&host, captured.clone(), || {
				sse_response(inline_sse_payload(None, small_usage()))
			});
			let session_id = "test-session-123";

			let model = gpt_51_codex();
			let context = say_hello_context(pi_host::now_ms());

			let stream_result = stream_openai_codex_responses(
				&model,
				&context,
				options(&token, |options| {
					options.session_id = Some(session_id.into());
					options.transport = Some(Transport::Sse);
				}),
			);
			result(stream_result).await;

			let captured = captured.borrow();
			assert_eq!(captured.len(), 1);
			let request = &captured[0];
			// Verify sessionId is set in headers
			assert_eq!(request.header("session-id"), Some(session_id));
			assert!(request.header("session_id").is_none());
			assert_eq!(request.header("x-client-request-id"), Some(session_id));

			// Verify sessionId is set in request body as prompt_cache_key
			let body = decode_codex_request_body(request).expect("request has a body");
			assert_eq!(body["prompt_cache_key"], json!(session_id));
		});
	}

	#[test]
	fn omits_sse_cache_affinity_when_cache_retention_is_none() {
		run_codex(|host| async move {
			let token = mock_token("acc_test");
			let captured = Rc::new(RefCell::new(Vec::new()));
			capturing_fetch(&host, captured.clone(), || {
				sse_response(build_sse_payload("completed", false, None))
			});

			let model = gpt_51_codex();
			let context = say_hello_context(pi_host::now_ms());

			result(stream_openai_codex_responses(
				&model,
				&context,
				options(&token, |options| {
					options.cache_retention = Some(CacheRetention::None);
					options.session_id = Some("one-off-summary".into());
					options.transport = Some(Transport::Sse);
				}),
			))
			.await;

			let captured = captured.borrow();
			let request = captured.last().expect("fetch was called");
			assert!(request.header("session-id").is_none());
			assert!(request.header("x-client-request-id").is_none());
			let body = decode_codex_request_body(request).expect("request has a body");
			assert!(body.get("prompt_cache_key").is_none());
		});
	}

	#[test]
	fn clamps_prompt_cache_key_to_openais_64_character_limit() {
		run_codex(|host| async move {
			let token = mock_token("acc_test");
			let session_id = "x".repeat(67);
			capturing_fetch(&host, Rc::default(), || {
				sse_response(build_sse_payload("completed", false, None))
			});

			let model = gpt_51_codex();
			let context = say_hello_context(pi_host::now_ms());
			let (on_payload, captured_payload) = payload_recorder();

			result(stream_openai_codex_responses(
				&model,
				&context,
				options(&token, |options| {
					options.transport = Some(Transport::Sse);
					options.session_id = Some(session_id);
					options.on_payload = Some(on_payload);
				}),
			))
			.await;

			assert_eq!(
				captured_payload
					.borrow()
					.as_ref()
					.map(|payload| payload["prompt_cache_key"].clone()),
				Some(json!("x".repeat(64)))
			);
		});
	}

	#[test]
	fn clamps_codex_session_id_header_to_64_characters() {
		run_codex(|host| async move {
			let token = mock_token("acc_test");
			let session_id = "x".repeat(67);
			let captured = Rc::new(RefCell::new(Vec::new()));
			capturing_fetch(&host, captured.clone(), || {
				sse_response(build_sse_payload("completed", false, None))
			});

			let model = gpt_51_codex();
			let context = say_hello_context(pi_host::now_ms());

			result(stream_openai_codex_responses(
				&model,
				&context,
				options(&token, |options| {
					options.transport = Some(Transport::Sse);
					options.session_id = Some(session_id);
				}),
			))
			.await;

			let captured = captured.borrow();
			let request = captured.last().expect("fetch was called");
			assert_eq!(request.header("session-id"), Some("x".repeat(64).as_str()));
			assert_eq!(request.header("x-client-request-id"), Some("x".repeat(64).as_str()));
		});
	}

	#[test]
	fn lets_model_and_caller_headers_override_originator_and_user_agent() {
		run_codex(|host| async move {
			let token = mock_token("acc_test");
			let captured = Rc::new(RefCell::new(Vec::new()));
			capturing_fetch(&host, captured.clone(), || {
				sse_response(build_sse_payload("completed", false, None))
			});

			let mut model = gpt_51_codex();
			model.headers = Some(headers(&[("originator", "my-app")]));
			let context = say_hello_context(pi_host::now_ms());

			result(stream_openai_codex_responses(
				&model,
				&context,
				options(&token, |options| {
					options.transport = Some(Transport::Sse);
					options.headers = Some(IndexMap::from([
						("user-agent".to_string(), Some("my-app/1.0".to_string())),
						("Authorization".to_string(), Some("Bearer ignored".to_string())),
					]));
				}),
			))
			.await;

			let captured = captured.borrow();
			let request = captured.last().expect("fetch was called");
			assert_eq!(request.header("originator"), Some("my-app"));
			assert_eq!(request.header("User-Agent"), Some("my-app/1.0"));
			assert_eq!(
				request.header("Authorization"),
				Some(format!("Bearer {token}").as_str())
			);
		});
	}

	#[test]
	fn preserves_gpt_5_5_xhigh_reasoning_effort_from_simple_options() {
		run_codex(|host| async move {
			let token = mock_token("acc_test");
			let captured = Rc::new(RefCell::new(Vec::new()));
			routed_fetch(&host, captured.clone(), || {
				sse_response(build_sse_payload("completed", false, None))
			});

			let mut model = codex_model("gpt-5.5", "GPT-5.5");
			model.thinking_level_map = Some(serde_json::from_value(json!({ "xhigh": "xhigh" })).unwrap());
			let context = say_hello_context(pi_host::now_ms());

			let simple_options = SimpleStreamOptions {
				base: StreamOptions {
					base: ProviderRequestOptions {
						api_key: Some(token.clone()),
						..ProviderRequestOptions::default()
					},
					transport: Some(Transport::Sse),
					..StreamOptions::default()
				},
				reasoning: Some(ThinkingLevel::Xhigh),
				..SimpleStreamOptions::default()
			};
			result(stream_simple_openai_codex_responses(&model, &context, Some(simple_options)).unwrap()).await;

			let captured = captured.borrow();
			let body = decode_codex_request_body(&captured[0]).expect("request has a body");
			assert_eq!(body["reasoning"], json!({ "effort": "xhigh", "summary": "auto" }));
		});
	}

	#[test]
	fn forwards_required_tool_choice() {
		run_codex(|host| async move {
			let token = mock_token("acc_test");
			let captured = Rc::new(RefCell::new(Vec::new()));
			capturing_fetch(&host, captured.clone(), || {
				sse_response(build_sse_payload("completed", false, None))
			});

			let model = codex_model("gpt-5.5", "GPT-5.5");

			result(stream_openai_codex_responses(
				&model,
				&normalize_context(
					&serde_json::from_value(json!({
						"messages": [
							{ "role": "user", "content": "Do not call ping. Respond with text instead.", "timestamp": pi_host::now_ms() },
						],
						"tools": [{
							"name": "ping",
							"description": "Ping",
							"parameters": {
								"type": "object",
								"properties": { "value": { "type": "string" } },
								"required": ["value"],
							},
						}],
					}))
					.unwrap(),
				),
				options(&token, |options| {
					options.transport = Some(Transport::Sse);
					options.tool_choice = Some(json!("required"));
				}),
			))
			.await;

			let captured = captured.borrow();
			let body = decode_codex_request_body(&captured[0]).expect("request has a body");
			assert_eq!(body["tool_choice"], json!("required"));
		});
	}

	#[test]
	fn sets_codex_strict_mode_explicitly_and_honors_constrained_sampling() {
		run_codex(|host| async move {
			let token = mock_token("acc_test");
			capturing_fetch(&host, Rc::default(), || {
				sse_response(build_sse_payload("completed", false, None))
			});

			let model = codex_model("gpt-5.5", "GPT-5.5");
			let context = normalize_context(
				&serde_json::from_value(json!({
					"messages": [{ "role": "user", "content": "Use a tool", "timestamp": pi_host::now_ms() }],
					"tools": [
						{
							"name": "optional",
							"description": "Optional constrained sampling",
							"parameters": {
								"type": "object",
								"properties": { "value": { "type": "string" } },
								"required": ["value"],
							},
							"constrainedSampling": false,
						},
						{
							"name": "strict",
							"description": "Strict constrained sampling",
							"parameters": {
								"additionalProperties": false,
								"type": "object",
								"properties": { "value": { "type": "string" } },
								"required": ["value"],
							},
							"constrainedSampling": { "type": "json_schema", "strict": "prefer" },
						},
					],
				}))
				.unwrap(),
			);
			let (on_payload, captured_payload) = payload_recorder();

			result(stream_openai_codex_responses(
				&model,
				&context,
				options(&token, |options| {
					options.transport = Some(Transport::Sse);
					options.on_payload = Some(on_payload);
				}),
			))
			.await;

			let payload = captured_payload.borrow().clone().expect("payload was captured");
			assert_match_object(
				&payload["tools"],
				&json!([
					{ "type": "function", "name": "optional", "strict": null },
					{ "type": "function", "name": "strict", "strict": true },
				]),
			);
		});
	}

	#[test]
	fn clamps_minimal_reasoning_effort_to_low() {
		for model_id in ["gpt-5.3-codex", "gpt-5.4", "gpt-5.5"] {
			run_codex(|host| async move {
				let token = mock_token("acc_test");
				let captured = Rc::new(RefCell::new(Vec::new()));
				routed_fetch(&host, captured.clone(), || {
					sse_response(inline_sse_payload(None, small_usage()))
				});

				let mut model = codex_model(model_id, model_id);
				model.thinking_level_map = Some(serde_json::from_value(json!({ "minimal": "low" })).unwrap());
				let context = say_hello_context(pi_host::now_ms());

				let stream_result = stream_openai_codex_responses(
					&model,
					&context,
					options(&token, |options| {
						options.reasoning_effort = Some("minimal".into());
						options.transport = Some(Transport::Sse);
					}),
				);
				result(stream_result).await;
				let captured = captured.borrow();
				let body = decode_codex_request_body(&captured[0]).expect("request has a body");
				assert_eq!(
					body["reasoning"],
					json!({ "effort": "low", "summary": "auto" }),
					"clamps {model_id} minimal reasoning effort to low"
				);
			});
		}
	}

	#[test]
	fn uses_the_client_sent_service_tier_when_codex_echoes_default() {
		for (model_id, service_tier, multiplier) in [
			("gpt-5.1-codex", "flex", 0.5),
			("gpt-5.1-codex", "priority", 2.0),
			("gpt-5.5", "flex", 0.5),
			("gpt-5.5", "priority", 2.5),
		] {
			run_codex(|host| async move {
				let token = mock_token("acc_test");
				let usage = json!({
					"input_tokens": 1000000,
					"output_tokens": 1000000,
					"total_tokens": 2000000,
					"input_tokens_details": { "cached_tokens": 0 },
				});
				routed_fetch(&host, Rc::default(), move || {
					sse_response(inline_sse_payload(Some("default"), usage.clone()))
				});

				let mut model = codex_model(
					model_id,
					if model_id == "gpt-5.5" {
						"GPT-5.5"
					} else {
						"GPT-5.1 Codex"
					},
				);
				model.cost =
					serde_json::from_value(json!({ "input": 1, "output": 2, "cacheRead": 0, "cacheWrite": 0 }))
						.unwrap();
				let context = say_hello_context(pi_host::now_ms());

				let result = result(stream_openai_codex_responses(
					&model,
					&context,
					options(&token, |options| {
						options.service_tier = Some(json!(service_tier));
						options.transport = Some(Transport::Sse);
					}),
				))
				.await;

				let case = format!("uses the client-sent {service_tier} service tier for {model_id}");
				assert_eq!(result.usage.cost.input, 1.0 * multiplier, "{case}");
				assert_eq!(result.usage.cost.output, 2.0 * multiplier, "{case}");
				assert_eq!(result.usage.cost.total, 3.0 * multiplier, "{case}");
			});
		}
	}

	#[test]
	fn does_not_set_session_id_x_client_request_id_headers_when_session_id_is_not_provided() {
		run_codex(|host| async move {
			let token = mock_token("acc_test");
			let captured = Rc::new(RefCell::new(Vec::new()));
			routed_fetch(&host, captured.clone(), || {
				sse_response(inline_sse_payload(None, small_usage()))
			});

			let model = gpt_51_codex();
			let context = say_hello_context(pi_host::now_ms());

			// No sessionId provided
			let stream_result = stream_openai_codex_responses(&model, &context, sse_options(&token));
			result(stream_result).await;

			let captured = captured.borrow();
			assert_eq!(captured.len(), 1);
			let request = &captured[0];
			// Verify headers are not set when sessionId is not provided
			assert!(request.header("session-id").is_none());
			assert!(request.header("session_id").is_none());
			assert!(request.header("x-client-request-id").is_none());
		});
	}

	#[test]
	fn forwards_auto_transport_and_raw_provider_events_from_stream_simple() {
		run_codex(|host| async move {
			let token = mock_token("acc_test");
			let sent_bodies: Rc<RefCell<Vec<Value>>> = Rc::default();
			let captured_websocket_headers: Rc<RefCell<Option<IndexMap<String, String>>>> = Rc::default();

			host.respond_with(500, &[], "unexpected fetch");

			let (sent, captured_headers) = (sent_bodies.clone(), captured_websocket_headers.clone());
			host.on_websocket(move |url, headers| {
				*captured_headers.borrow_mut() = Some(headers.clone());
				let socket = opening_socket(url, headers);
				let sent = sent.clone();
				socket.on_send(move |socket, data| {
					sent.borrow_mut().push(serde_json::from_str(data).unwrap());
					let events = [
						json!({
							"type": "response.output_item.added",
							"item": { "type": "message", "id": "msg_1", "role": "assistant", "status": "in_progress", "content": [] },
						}),
						json!({ "type": "response.content_part.added", "part": { "type": "output_text", "text": "" } }),
						json!({ "type": "response.output_text.delta", "delta": "Hello" }),
						json!({
							"type": "response.output_item.done",
							"item": {
								"type": "message",
								"id": "msg_1",
								"role": "assistant",
								"status": "completed",
								"content": [{ "type": "output_text", "text": "Hello" }],
							},
						}),
						json!({
							"type": "response.done",
							"response": { "status": "completed", "end_turn": false, "usage": small_usage() },
						}),
					];
					socket.emit_later(message_events(&events));
					Ok(())
				});
				Ok(socket)
			});

			let model = gpt_51_codex();
			let context = say_hello_context(1.0);
			let (on_provider_stream_event, provider_events, provider_event_models) = provider_event_recorder();

			let simple_options = SimpleStreamOptions {
				base: StreamOptions {
					base: ProviderRequestOptions {
						api_key: Some(token.clone()),
						..ProviderRequestOptions::default()
					},
					session_id: Some("session-auto".into()),
					transport: Some(Transport::Auto),
					on_provider_stream_event: Some(on_provider_stream_event),
					..StreamOptions::default()
				},
				..SimpleStreamOptions::default()
			};
			let result =
				result(stream_simple_openai_codex_responses(&model, &context, Some(simple_options)).unwrap()).await;

			assert_eq!(result.end_turn, Some(false));
			assert_eq!(sent_bodies.borrow().len(), 1);
			assert_eq!(
				event_types(&provider_events.borrow()),
				[
					"response.output_item.added",
					"response.content_part.added",
					"response.output_text.delta",
					"response.output_item.done",
					"response.done",
				]
			);
			assert_eq!(*provider_event_models.borrow(), vec![model.clone(); 5]);
			let websocket_headers = captured_websocket_headers
				.borrow()
				.clone()
				.expect("websocket was opened");
			assert_eq!(
				websocket_headers.get("session-id").map(String::as_str),
				Some("session-auto")
			);
			assert!(!websocket_headers.contains_key("session_id"));
			assert_eq!(
				websocket_headers.get("x-client-request-id").map(String::as_str),
				Some("session-auto")
			);
			assert!(host.requests().is_empty());
			let stats = get_openai_codex_websocket_debug_stats("session-auto").expect("stats exist");
			assert_eq!(stats.cached_context_requests, 1);
			assert_eq!(stats.full_context_requests, 1);
		});
	}

	#[test]
	fn scopes_cached_websockets_to_the_authenticated_account() {
		// Regression for #7284: rotating accounts must not reuse a socket authenticated by another account.
		run_codex(|host| async move {
			let connected_headers: Rc<RefCell<Vec<IndexMap<String, String>>>> = Rc::default();
			let response_id = Rc::new(Cell::new(0));

			let (connected, response_id_counter) = (connected_headers.clone(), response_id.clone());
			host.on_websocket(move |url, headers| {
				connected.borrow_mut().push(headers.clone());
				let socket = ready_socket(url, headers);
				let response_id = response_id_counter.clone();
				socket.on_send(move |socket, _data| {
					response_id.set(response_id.get() + 1);
					socket.emit_later(vec![message_event(&json!({
						"type": "response.completed",
						"response": {
							"id": format!("resp_{}", response_id.get()),
							"status": "completed",
							"usage": { "input_tokens": 1, "output_tokens": 1, "total_tokens": 2 },
						},
					}))]);
					Ok(())
				});
				Ok(socket)
			});
			host.respond_with(500, &[], "unexpected fetch");

			let model = gpt_51_codex();
			let context = context(Some(""), Vec::new());
			let with_account = |account_id: &str| {
				options(&mock_token(account_id), |options| {
					options.session_id = Some("shared-session".into());
					options.transport = Some(Transport::WebsocketCached);
				})
			};

			result(stream_openai_codex_responses(
				&model,
				&context,
				with_account("account-a"),
			))
			.await;
			result(stream_openai_codex_responses(
				&model,
				&context,
				with_account("account-b"),
			))
			.await;
			result(stream_openai_codex_responses(
				&model,
				&context,
				with_account("account-a"),
			))
			.await;

			let connected = connected_headers.borrow();
			assert_eq!(
				connected
					.iter()
					.map(|headers| headers.get("chatgpt-account-id").cloned())
					.collect::<Vec<_>>(),
				[Some("account-a".to_string()), Some("account-b".to_string())]
			);
			assert_eq!(
				connected
					.iter()
					.map(|headers| headers.get("authorization").cloned())
					.collect::<Vec<_>>(),
				[
					Some(format!("Bearer {}", mock_token("account-a"))),
					Some(format!("Bearer {}", mock_token("account-b"))),
				]
			);
			assert!(host.requests().is_empty());
			let stats = get_openai_codex_websocket_debug_stats("shared-session").expect("stats exist");
			assert_eq!(stats.connections_created, 2);
			assert_eq!(stats.connections_reused, 1);
		});
	}

	#[test]
	fn closes_one_shot_websockets_when_cache_retention_is_none() {
		run_codex(|host| async move {
			let token = mock_token("acc_test");
			let sent_bodies: Rc<RefCell<Vec<Value>>> = Rc::default();
			let connections = Rc::new(Cell::new(0));
			let closed_connections = Rc::new(Cell::new(0));

			let (sent, connection_count, closed_count) =
				(sent_bodies.clone(), connections.clone(), closed_connections.clone());
			host.on_websocket(move |url, headers| {
				connection_count.set(connection_count.get() + 1);
				let socket = opening_socket(url, headers);
				let (sent, connection_count, closed_count) =
					(sent.clone(), connection_count.clone(), closed_count.clone());
				socket.on_send(move |socket, data| {
					sent.borrow_mut().push(serde_json::from_str(data).unwrap());
					socket.emit_later(vec![message_event(&json!({
						"type": "response.completed",
						"response": {
							"id": format!("resp_{}", connection_count.get()),
							"status": "completed",
							"usage": { "input_tokens": 5, "output_tokens": 3, "total_tokens": 8 },
						},
					}))]);
					Ok(())
				});
				socket.on_close(move |_socket, _code, _reason| closed_count.set(closed_count.get() + 1));
				Ok(socket)
			});
			host.respond_with(500, &[], "unexpected fetch");

			let model = gpt_51_codex();
			let context = say_hello_context(1.0);
			let one_off = || {
				options(&token, |options| {
					options.cache_retention = Some(CacheRetention::None);
					options.session_id = Some("one-off-summary".into());
					options.transport = Some(Transport::Auto);
				})
			};

			result(stream_openai_codex_responses(&model, &context, one_off())).await;
			result(stream_openai_codex_responses(&model, &context, one_off())).await;

			assert_eq!(connections.get(), 2);
			assert_eq!(closed_connections.get(), 2);
			assert_eq!(sent_bodies.borrow().len(), 2);
			assert!(
				sent_bodies
					.borrow()
					.iter()
					.all(|body| body.get("prompt_cache_key").is_none())
			);
			assert!(get_openai_codex_websocket_debug_stats("one-off-summary").is_none());
			assert!(host.requests().is_empty());
		});
	}

	#[test]
	fn falls_back_to_sse_when_websocket_connect_does_not_open_before_the_connect_timeout() {
		run_codex(|host| async move {
			let token = mock_token("acc_test");
			let fetch_calls = Rc::new(Cell::new(0));
			codex_only_fetch(&host, fetch_calls.clone());

			host.on_websocket(|url, headers| {
				let socket = FakeWebSocket::new(url, headers);
				socket.on_send(|_socket, _data| Err(PiError::new("send should not be called before websocket open")));
				Ok(socket)
			});

			let model = gpt_51_codex();
			let context = say_hello_context(1.0);

			let result = result(stream_openai_codex_responses(
				&model,
				&context,
				options(&token, |options| {
					options.session_id = Some("ws-connect-timeout".into());
					options.transport = Some(Transport::Auto);
					options.timeout_ms = Some(300_000.0);
					options.websocket_connect_timeout_ms = Some(50.0);
				}),
			))
			.await;

			assert_eq!(text_of(&result).as_deref(), Some("Hello"));
			assert_eq!(fetch_calls.get(), 1);
			let stats = get_openai_codex_websocket_debug_stats("ws-connect-timeout").expect("stats exist");
			assert_eq!(stats.websocket_failures, 1);
			assert_eq!(stats.sse_fallbacks, 1);
			assert_eq!(stats.websocket_fallback_active, Some(true));
			assert_eq!(
				stats.last_websocket_error.as_deref(),
				Some("WebSocket connect timeout after 50ms")
			);
		});
	}

	#[test]
	fn reconnects_once_when_the_websocket_connection_limit_is_reached_before_output_starts() {
		run_codex(|host| async move {
			let token = mock_token("acc_test");
			let connections = Rc::new(Cell::new(0));

			host.respond_with(500, &[], "unexpected fetch");

			let connection_count = connections.clone();
			host.on_websocket(move |url, headers| {
				let limit_reached = connection_count.get() == 0;
				connection_count.set(connection_count.get() + 1);
				let socket = opening_socket(url, headers);
				socket.on_send(move |socket, _data| {
					let event = if limit_reached {
						json!({ "type": "error", "error": { "code": "websocket_connection_limit_reached" } })
					} else {
						json!({
							"type": "response.completed",
							"response": {
								"id": "resp_1",
								"status": "completed",
								"usage": { "input_tokens": 5, "output_tokens": 3, "total_tokens": 8 },
							},
						})
					};
					socket.emit_later(vec![message_event(&event)]);
					Ok(())
				});
				Ok(socket)
			});

			let model = gpt_51_codex();

			let result = result(stream_openai_codex_responses(
				&model,
				&context(Some(""), Vec::new()),
				options(&token, |_| {}),
			))
			.await;

			assert_eq!(result.stop_reason, StopReason::Stop);
			assert_eq!(connections.get(), 2);
			assert!(host.requests().is_empty());
		});
	}

	#[test]
	fn falls_back_to_sse_when_a_websocket_is_idle_before_the_first_event() {
		run_codex(|host| async move {
			let token = mock_token("acc_test");
			let sent_bodies: Rc<RefCell<Vec<Value>>> = Rc::default();
			let fetch_calls = Rc::new(Cell::new(0));
			codex_only_fetch(&host, fetch_calls.clone());

			let sent = sent_bodies.clone();
			host.on_websocket(move |url, headers| {
				let socket = ready_socket(url, headers);
				let sent = sent.clone();
				socket.on_send(move |_socket, data| {
					sent.borrow_mut().push(serde_json::from_str(data).unwrap());
					Ok(())
				});
				Ok(socket)
			});

			let model = gpt_51_codex();
			let context = say_hello_context(1.0);

			let result_stream = stream_openai_codex_responses(
				&model,
				&context,
				options(&token, |options| {
					options.session_id = Some("ws-idle-before-start".into());
					options.transport = Some(Transport::Auto);
					options.timeout_ms = Some(50.0);
				}),
			);
			let check_sent = async {
				pi_host::sleep(0.0, None).await.unwrap();
				assert_eq!(sent_bodies.borrow().len(), 1);
			};
			let (result, ()) = futures::join!(result(result_stream), check_sent);

			assert_eq!(text_of(&result).as_deref(), Some("Hello"));
			assert_eq!(fetch_calls.get(), 1);
			let stats = get_openai_codex_websocket_debug_stats("ws-idle-before-start").expect("stats exist");
			assert_eq!(stats.websocket_failures, 1);
			assert_eq!(stats.sse_fallbacks, 1);
			assert_eq!(stats.websocket_fallback_active, Some(true));
		});
	}

	#[test]
	fn errors_when_a_websocket_is_idle_after_the_stream_started() {
		run_codex(|host| async move {
			let token = mock_token("acc_test");
			host.respond_with(500, &[], "unexpected fetch");

			host.on_websocket(|url, headers| {
				let socket = ready_socket(url, headers);
				socket.on_send(|socket, _data| {
					socket.emit_later(vec![message_event(&json!({
						"type": "response.output_item.added",
						"item": { "type": "message", "id": "msg_1", "role": "assistant", "status": "in_progress", "content": [] },
					}))]);
					Ok(())
				});
				Ok(socket)
			});

			let model = gpt_51_codex();
			let context = say_hello_context(1.0);

			let result = result(stream_openai_codex_responses(
				&model,
				&context,
				options(&token, |options| {
					options.transport = Some(Transport::Auto);
					options.timeout_ms = Some(50.0);
				}),
			))
			.await;

			assert_eq!(result.stop_reason, StopReason::Error);
			assert_eq!(
				result.error_message.as_deref(),
				Some("WebSocket idle timeout after 50ms")
			);
			assert!(host.requests().is_empty());
		});
	}

	#[test]
	fn opens_a_fresh_cached_websocket_before_the_backend_connection_age_limit() {
		run_codex(|host| async move {
			// 2026-07-03T00:00:00Z
			let started_at = 1_783_036_800_000.0;
			set_system_time(&host, started_at);
			let token = mock_token("acc_test");
			let sent_connection_ids: Rc<RefCell<Vec<u32>>> = Rc::default();
			let connections = Rc::new(Cell::new(0u32));

			let (sent, connection_count) = (sent_connection_ids.clone(), connections.clone());
			host.on_websocket(move |url, headers| {
				connection_count.set(connection_count.get() + 1);
				let connection_id = connection_count.get();
				let socket = ready_socket(url, headers);
				let sent = sent.clone();
				socket.on_send(move |socket, _data| {
					sent.borrow_mut().push(connection_id);
					socket.emit_later(vec![message_event(&json!({
						"type": "response.completed",
						"response": {
							"id": format!("resp_{connection_id}"),
							"status": "completed",
							"usage": { "input_tokens": 5, "output_tokens": 3, "total_tokens": 8 },
						},
					}))]);
					Ok(())
				});
				Ok(socket)
			});

			let model = gpt_51_codex();
			let session_id = "aged-ws-session";
			let first_context = say_hello_context(1.0);
			let cached = || {
				options(&token, |options| {
					options.session_id = Some(session_id.into());
					options.transport = Some(Transport::WebsocketCached);
				})
			};

			let first = result(stream_openai_codex_responses(&model, &first_context, cached())).await;
			set_system_time(&host, started_at + 56.0 * 60.0 * 1000.0);
			let mut messages = first_context.messages.clone();
			messages.push(Message::Assistant(first));
			messages.push(user_message("Now finish", 2.0));
			let second_context = context(None, messages);

			result(stream_openai_codex_responses(&model, &second_context, cached())).await;

			assert_eq!(connections.get(), 2);
			assert_eq!(*sent_connection_ids.borrow(), [1, 2]);
			let stats = get_openai_codex_websocket_debug_stats(session_id).expect("stats exist");
			assert_eq!(stats.connections_created, 2);
			assert_eq!(stats.connections_reused, 0);
		});
	}

	#[test]
	fn sends_only_response_input_deltas_in_websocket_cached_mode() {
		run_codex(|host| async move {
			let token = mock_token("acc_test");
			let sent_bodies: Rc<RefCell<Vec<Value>>> = Rc::default();

			let sent = sent_bodies.clone();
			host.on_websocket(move |url, headers| {
				let socket = ready_socket(url, headers);
				let sent = sent.clone();
				socket.on_send(move |socket, data| {
					sent.borrow_mut().push(serde_json::from_str(data).unwrap());
					let sent_count = sent.borrow().len();
					let response_id = format!("resp_{sent_count}");
					let mut events = vec![json!({ "type": "response.created", "response": { "id": response_id } })];
					if sent_count == 1 {
						events.extend([
							json!({
								"type": "response.output_item.added",
								"item": {
									"type": "custom_tool_call",
									"id": "ctc_1",
									"call_id": "call_1",
									"name": "sample_tool",
									"input": "",
								},
							}),
							json!({ "type": "response.custom_tool_call_input.delta", "item_id": "ctc_1", "delta": "abc" }),
							json!({ "type": "response.custom_tool_call_input.done", "item_id": "ctc_1", "input": "abc" }),
							json!({
								"type": "response.output_item.done",
								"item": {
									"type": "custom_tool_call",
									"id": "ctc_1",
									"call_id": "call_1",
									"name": "sample_tool",
									"input": "abc",
								},
							}),
						]);
					}
					events.push(json!({
						"type": "response.completed",
						"response": { "id": response_id, "status": "completed", "usage": small_usage() },
					}));
					socket.emit_later(message_events(&events));
					Ok(())
				});
				Ok(socket)
			});

			let mut model = gpt_51_codex();
			model.compat = Some(
				json!({ "supportsOpenAIGrammarTools": true })
					.as_object()
					.cloned()
					.unwrap(),
			);
			let first_context: Context = serde_json::from_value(json!({
				"systemPrompt": "You are a helpful assistant.",
				"messages": [{ "role": "user", "content": "Use the tool", "timestamp": 1 }],
				"tools": [{
					"name": "sample_tool",
					"description": "Sample tool",
					"parameters": {
						"type": "object",
						"properties": { "payload": { "type": "string" } },
						"required": ["payload"],
					},
					"constrainedSampling": { "type": "grammar", "variants": { "openai_lark": "start: /[a-z]+/" } },
				}],
			}))
			.unwrap();
			let cached = || {
				options(&token, |options| {
					options.session_id = Some("session-1".into());
					options.transport = Some(Transport::WebsocketCached);
				})
			};

			let first = result(stream_openai_codex_responses(
				&model,
				&normalize_context(&first_context),
				cached(),
			))
			.await;

			let mut messages = first_context.messages.clone();
			messages.push(Message::Assistant(first));
			messages.push(
				serde_json::from_value(json!({
					"role": "toolResult",
					"toolCallId": "call_1|ctc_1",
					"toolName": "sample_tool",
					"content": [{ "type": "text", "text": "real result" }],
					"isError": false,
					"timestamp": 2,
				}))
				.unwrap(),
			);
			messages.push(user_message("Now finish", 3.0));
			let second_context = normalize_context(&Context {
				messages,
				..first_context.clone()
			});
			result(stream_openai_codex_responses(&model, &second_context, cached())).await;

			let sent_bodies = sent_bodies.borrow();
			assert_eq!(sent_bodies.len(), 2);
			let (first_body, second_body) = (&sent_bodies[0], &sent_bodies[1]);
			assert_eq!(first_body["store"], json!(false));
			assert!(first_body.get("previous_response_id").is_none());
			assert_eq!(
				first_body["input"],
				json!([{ "role": "user", "content": [{ "type": "input_text", "text": "Use the tool" }] }])
			);
			assert_eq!(second_body["store"], json!(false));
			assert_eq!(second_body["previous_response_id"], json!("resp_1"));
			assert_eq!(
				second_body["input"],
				json!([
					{ "type": "custom_tool_call_output", "call_id": "call_1", "output": "real result" },
					{ "role": "user", "content": [{ "type": "input_text", "text": "Now finish" }] },
				])
			);
			let stats = get_openai_codex_websocket_debug_stats("session-1").expect("stats exist");
			assert_eq!(stats.requests, 2);
			assert_eq!(stats.connections_created, 1);
			assert_eq!(stats.connections_reused, 1);
			assert_eq!(stats.cached_context_requests, 2);
			assert_eq!(stats.store_true_requests, 0);
			assert_eq!(stats.full_context_requests, 1);
			assert_eq!(stats.delta_requests, 1);
			assert_eq!(stats.last_delta_input_items, Some(2));
			assert_eq!(stats.last_previous_response_id.as_deref(), Some("resp_1"));
		});
	}

	#[test]
	fn recovers_a_missing_cached_websocket_continuation() {
		for recovery_transport in ["websocket", "sse"] {
			run_codex(|host| async move {
				let token = mock_token("acc_test");
				let session_id = format!("missing-continuation-{recovery_transport}");
				let fetch_calls = Rc::new(Cell::new(0));
				let counted = fetch_calls.clone();
				host.on_fetch(move |_| {
					counted.set(counted.get() + 1);
					Box::pin(async { Ok(sse_response(build_sse_payload("completed", false, None))) })
				});
				let sent_bodies: Rc<RefCell<Vec<(u32, Value)>>> = Rc::default();
				let connections = Rc::new(Cell::new(0u32));

				let (sent, connection_count) = (sent_bodies.clone(), connections.clone());
				host.on_websocket(move |url, headers| {
					connection_count.set(connection_count.get() + 1);
					let connection_id = connection_count.get();
					let socket = ready_socket(url, headers);
					let sent = sent.clone();
					socket.on_send(move |socket, data| {
						sent.borrow_mut()
							.push((connection_id, serde_json::from_str(data).unwrap()));
						let sent_count = sent.borrow().len();
						if sent_count == 2 {
							socket.emit_later(message_events(&[
								json!({
									"type": "codex.rate_limits",
									"plan_type": "plus",
									"rate_limits": {
										"allowed": true,
										"limit_reached": false,
										"primary": {
											"used_percent": 7,
											"window_minutes": 10080,
											"reset_after_seconds": 556112,
											"reset_at": 1785269351_i64,
										},
										"secondary": null,
									},
									"code_review_rate_limits": null,
									"additional_rate_limits": null,
									"credits": { "has_credits": false, "unlimited": false, "balance": "0" },
									"promo": null,
								}),
								json!({
									"type": "error",
									"status": 400,
									"error": {
										"code": "previous_response_not_found",
										"message": "Previous response with id 'resp_1' not found.",
										"param": "previous_response_id",
									},
								}),
							]));
							return Ok(());
						}
						if sent_count == 3 && recovery_transport == "sse" {
							socket.emit_later(vec![WebSocketEvent::Error {
								message: Some("retry websocket failed".into()),
							}]);
							return Ok(());
						}

						let (response_id, message_id, text) = if sent_count == 1 {
							("resp_1", "msg_1", "Hello")
						} else {
							("resp_2", "msg_2", "Recovered")
						};
						socket.emit_later(message_events(&[
							json!({ "type": "response.created", "response": { "id": response_id } }),
							json!({
								"type": "response.output_item.added",
								"output_index": 0,
								"item": {
									"type": "message",
									"id": message_id,
									"role": "assistant",
									"status": "in_progress",
									"content": [],
								},
							}),
							json!({
								"type": "response.output_item.done",
								"output_index": 0,
								"item": {
									"type": "message",
									"id": message_id,
									"role": "assistant",
									"status": "completed",
									"content": [{ "type": "output_text", "text": text }],
								},
							}),
							json!({
								"type": "response.completed",
								"response": {
									"id": response_id,
									"status": "completed",
									"usage": { "input_tokens": 5, "output_tokens": 3, "total_tokens": 8 },
								},
							}),
						]));
						Ok(())
					});
					Ok(socket)
				});

				let model = gpt_51_codex();
				let first_context = say_hello_context(1.0);
				let cached = || {
					options(&token, |options| {
						options.session_id = Some(session_id.clone());
						options.transport = Some(Transport::WebsocketCached);
					})
				};

				let first = result(stream_openai_codex_responses(&model, &first_context, cached())).await;
				let mut messages = first_context.messages.clone();
				messages.push(Message::Assistant(first));
				messages.push(user_message("Now finish", 2.0));
				let second_context = context(None, messages);
				let mut event_types: Vec<&'static str> = Vec::new();
				let second_stream = stream_openai_codex_responses(&model, &second_context, cached());
				while let Some(event) = second_stream.next().await {
					event_types.push(event_name(&event));
				}
				let second = result(second_stream).await;

				let case = format!("recovers a missing cached websocket continuation via {recovery_transport}");
				let fallbacks = if recovery_transport == "sse" { 1 } else { 0 };
				assert_eq!(second.stop_reason, StopReason::Stop, "{case}");
				assert_eq!(
					text_of(&second).as_deref(),
					Some(if recovery_transport == "sse" {
						"Hello"
					} else {
						"Recovered"
					}),
					"{case}"
				);
				assert_eq!(
					event_types.iter().filter(|event| **event == "start").count(),
					1,
					"{case}"
				);
				assert!(!event_types.contains(&"error"), "{case}");
				assert_eq!(connections.get(), 2, "{case}");
				let sent_bodies = sent_bodies.borrow();
				assert_eq!(sent_bodies.len(), 3, "{case}");
				assert_eq!(
					sent_bodies
						.iter()
						.map(|(connection_id, _)| *connection_id)
						.collect::<Vec<_>>(),
					[1, 1, 2],
					"{case}"
				);
				assert_eq!(sent_bodies[1].1["previous_response_id"], json!("resp_1"), "{case}");
				assert_eq!(
					sent_bodies[1].1["input"],
					json!([{ "role": "user", "content": [{ "type": "input_text", "text": "Now finish" }] }]),
					"{case}"
				);
				assert!(sent_bodies[2].1.get("previous_response_id").is_none(), "{case}");
				let third_input = sent_bodies[2].1["input"].as_array().expect("input is an array");
				assert_eq!(third_input.len(), 3, "{case}");
				assert_eq!(
					third_input.last(),
					Some(&json!({ "role": "user", "content": [{ "type": "input_text", "text": "Now finish" }] })),
					"{case}"
				);
				assert_eq!(fetch_calls.get(), fallbacks, "{case}");
				let stats = get_openai_codex_websocket_debug_stats(&session_id).expect("stats exist");
				assert_eq!(stats.requests, 3, "{case}");
				assert_eq!(stats.connections_created, 2, "{case}");
				assert_eq!(stats.connections_reused, 1, "{case}");
				assert_eq!(stats.full_context_requests, 2, "{case}");
				assert_eq!(stats.delta_requests, 1, "{case}");
				assert_eq!(stats.websocket_failures, fallbacks as u64, "{case}");
				assert_eq!(stats.sse_fallbacks, fallbacks as u64, "{case}");
			});
		}
	}

	#[test]
	fn uses_retry_headers_for_sse_retries() {
		// `new Date(Date.now() + 45_000).toUTCString()` at the 2026-05-13T00:00:00Z system time.
		type RetryCase = (&'static str, [(&'static str, &'static str); 2], f64);
		let cases: [RetryCase; 3] = [
			(
				"retry-after-ms",
				[("content-type", "application/json"), ("retry-after-ms", "1500")],
				1500.0,
			),
			(
				"retry-after seconds",
				[("content-type", "application/json"), ("retry-after", "60")],
				60_000.0,
			),
			(
				"retry-after HTTP date",
				[
					("content-type", "application/json"),
					("retry-after", "Wed, 13 May 2026 00:00:45 GMT"),
				],
				45_000.0,
			),
		];
		for (name, retry_headers, expected_delay) in cases {
			run_codex(|host| async move {
				set_system_time(&host, 1_778_630_400_000.0);
				let token = mock_token("acc_test");
				let request_times = retrying_fetch(&host, 1, move || rate_limited(&retry_headers));

				let model = gpt_51_codex();
				let context = say_hello_context(pi_host::now_ms());

				let result = result(stream_openai_codex_responses(
					&model,
					&context,
					options(&token, |options| {
						options.transport = Some(Transport::Sse);
						options.max_retries = Some(1.0);
					}),
				))
				.await;

				let case = format!("uses {name} for SSE retries");
				assert_eq!(delays(&request_times.borrow()), [expected_delay], "{case}");
				assert_eq!(text_of(&result).as_deref(), Some("Hello"), "{case}");
				assert_eq!(request_times.borrow().len(), 2, "{case}");
			});
		}
	}

	#[test]
	fn fails_immediately_when_a_retry_delay_exceeds_the_limit() {
		for status in [429, 503] {
			run_codex(|host| async move {
				let token = mock_token("acc_test");
				let fetch_calls = Rc::new(Cell::new(0));
				let counted = fetch_calls.clone();
				host.on_fetch(move |_| {
					counted.set(counted.get() + 1);
					let response = text_response(
						status,
						&[("content-type", "application/json"), ("retry-after", "2")],
						json_stringify(
							&json!({ "error": { "code": "temporarily_unavailable", "message": "retry later" } }),
						),
					);
					Box::pin(async move { Ok(response) })
				});

				let model = gpt_51_codex();
				let context = say_hello_context(pi_host::now_ms());

				let result = result(stream_openai_codex_responses(
					&model,
					&context,
					options(&token, |options| {
						options.transport = Some(Transport::Sse);
						options.max_retries = Some(3.0);
						options.max_retry_delay_ms = Some(1000.0);
					}),
				))
				.await;

				let case = format!("fails immediately when a {status} retry delay exceeds the limit");
				assert_eq!(result.stop_reason, StopReason::Error, "{case}");
				assert_eq!(
					result.error_message.as_deref(),
					Some("Server requested 2s retry delay (max: 1s)"),
					"{case}"
				);
				assert_eq!(fetch_calls.get(), 1, "{case}");
			});
		}
	}

	#[test]
	#[cfg(feature = "native")]
	fn zstd_compresses_sse_request_bodies() {
		run_codex(|host| async move {
			let token = mock_token("acc_test");
			let captured = Rc::new(RefCell::new(Vec::new()));
			let recorded = captured.clone();
			host.on_fetch(move |request| {
				if request.url != "https://chatgpt.com/backend-api/codex/responses" {
					let url = request.url.clone();
					return Box::pin(async move { Err(PiError::new(format!("Unexpected URL: {url}"))) });
				}
				recorded.borrow_mut().push(request);
				Box::pin(async { Ok(sse_response(build_sse_payload("completed", false, None))) })
			});

			let model = gpt_51_codex();

			let large_text = "compress me ".repeat(400);
			result(stream_openai_codex_responses(
				&model,
				&context(
					Some("You are a helpful assistant."),
					vec![user_message(&large_text, 1.0)],
				),
				sse_options(&token),
			))
			.await;

			{
				let captured = captured.borrow();
				let request = captured.last().expect("fetch was called");
				assert_eq!(request.header("content-encoding"), Some("zstd"));
				let body = request.body.as_deref().expect("request has a body");
				assert!(serde_json::from_slice::<Value>(body).is_err());
				let decoded: Value = serde_json::from_slice(&zstd_decompress(body)).unwrap();
				assert_eq!(decoded["input"][0]["content"][0]["text"], json!(large_text));
			}

			captured.borrow_mut().clear();
			result(stream_openai_codex_responses(
				&model,
				&context(Some("You are a helpful assistant."), vec![user_message("hi", 1.0)]),
				sse_options(&token),
			))
			.await;

			let captured = captured.borrow();
			let request = captured.last().expect("fetch was called");
			assert_eq!(request.header("content-encoding"), Some("zstd"));
			let body = request.body.as_deref().expect("request has a body");
			assert!(serde_json::from_slice::<Value>(body).is_err());
		});
	}

	#[test]
	fn uses_exponential_backoff_across_repeated_sse_retries_without_retry_headers() {
		run_codex(|host| async move {
			set_system_time(&host, 1_778_630_400_000.0);
			let token = mock_token("acc_test");
			let request_times = retrying_fetch(&host, 3, || rate_limited(&[("content-type", "application/json")]));

			let model = gpt_51_codex();
			let context = say_hello_context(pi_host::now_ms());

			let result = result(stream_openai_codex_responses(
				&model,
				&context,
				options(&token, |options| {
					options.transport = Some(Transport::Sse);
					options.max_retries = Some(3.0);
				}),
			))
			.await;

			let delays = delays(&request_times.borrow());
			assert_eq!(delays[..1], [1000.0]);
			assert_eq!(delays[..2], [1000.0, 2000.0]);
			assert_eq!(delays, [1000.0, 2000.0, 4000.0]);
			assert_eq!(text_of(&result).as_deref(), Some("Hello"));
			assert_eq!(request_times.borrow().len(), 4);
		});
	}
}


/// Rust-only regressions from the adversarial review of `openai-codex-responses.ts`. The expected values come from
/// running the same requests through the TS adapter under vitest with a stubbed `fetch`.
mod review_regressions {
	use super::*;

	/// One SSE request with `maxRetries: 1`: the first Codex response is `status` with `body`, the next the SSE payload.
	/// Returns the result and the number of Codex requests.
	async fn first_response_fails(
		host: &TestHost,
		status: u16,
		retry_after: Option<&'static str>,
		body: Value,
		max_retry_delay_ms: Option<f64>,
	) -> (AssistantMessage, usize) {
		let token = mock_token("acc_test");
		let fetch_calls = Rc::new(Cell::new(0));
		let counted = fetch_calls.clone();
		let body = json_stringify(&body);
		host.on_fetch(move |_| {
			counted.set(counted.get() + 1);
			let mut headers = vec![("content-type", "application/json")];
			headers.extend(retry_after.map(|value| ("retry-after", value)));
			let response = if counted.get() == 1 {
				text_response(status, &headers, body.clone())
			} else {
				sse_response(build_sse_payload("completed", false, None))
			};
			Box::pin(async move { Ok(response) })
		});
		let model = gpt_51_codex();
		let context = say_hello_context(pi_host::now_ms());
		let result = result(stream_openai_codex_responses(
			&model,
			&context,
			options(&token, |options| {
				options.transport = Some(Transport::Sse);
				options.max_retries = Some(1.0);
				options.max_retry_delay_ms = max_retry_delay_ms;
			}),
		))
		.await;
		(result, fetch_calls.get())
	}

	// TS `/rate.?limit/i` has no `u` flag, so `.` is one UTF-16 code unit and a character outside the BMP never matches:
	// that 400 is not retryable, ignores `retry-after` and takes the network-error retry instead.
	#[test]
	fn retry_text_wildcard_matches_one_utf16_code_unit() {
		run_codex(|host| async move {
			let body = json!({ "error": { "code": "bad", "message": "rate\u{1F600}limit" } });
			let (result, calls) = first_response_fails(&host, 400, Some("2"), body, Some(1000.0)).await;
			assert_eq!(result.stop_reason, StopReason::Stop);
			assert_eq!(text_of(&result).as_deref(), Some("Hello"));
			assert_eq!(calls, 2);
		});
		run_codex(|host| async move {
			let body = json!({ "error": { "code": "bad", "message": "rate\u{e9}limit" } });
			let (result, calls) = first_response_fails(&host, 400, Some("2"), body, Some(1000.0)).await;
			assert_eq!(result.stop_reason, StopReason::Error);
			assert_eq!(result.error_message.as_deref(), Some("Server requested 2s retry delay (max: 1s)"));
			assert_eq!(calls, 1);
		});
	}

	// `Math.round` of an odd integer above 2^52 is the integer itself; `floor(v + 0.5)` rounds `v + 0.5` to even first.
	#[test]
	fn usage_limit_minutes_round_like_math_round_above_2_pow_52() {
		run_codex(|host| async move {
			assert_eq!(pi_host::now_ms(), 1_700_000_000_000.0);
			let body = json!({ "error": { "code": "usage_limit_reached", "resets_at": 3e17 } });
			let (result, calls) = first_response_fails(&host, 400, None, body, None).await;
			assert_eq!(
				result.error_message.as_deref(),
				Some("You have hit your ChatGPT usage limit. Try again in ~4999999971666667 min.")
			);
			assert_eq!(calls, 1);
		});
	}
}
