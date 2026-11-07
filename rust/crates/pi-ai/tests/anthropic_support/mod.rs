//! Shared fakes for the Anthropic Messages suites: SSE responses, a fake SDK client, and model and context builders.
//!
//! The TS suites pass `client: { beta: { messages: { create } } }` fakes; here the fake implements
//! `AnthropicClientApi` and records every params object it receives.

#![allow(dead_code)]

use std::cell::RefCell;
use std::rc::Rc;

use indexmap::IndexMap;
use pi_ai::api::anthropic_client::{AnthropicClientApi, RequestOptions};
use pi_ai::providers::all::get_builtin_model;
use pi_ai::types::{Context, Model, TranscriptContext};
use pi_ai::utils::transcript::normalize_context;
use pi_host::js::json_stringify;
use pi_host::{HttpResponse, LocalBoxFuture, PiError, PiResult};
use serde_json::{Value, json};

/// One SSE event: its `event` name and `data` text.
pub type SseEvent = (String, String);

/// `{ event, data: JSON.stringify(data) }`.
pub fn event(name: &str, data: Value) -> SseEvent {
	(name.to_string(), json_stringify(&data))
}

/// `{ event, data }` with raw data text.
pub fn raw_event(name: &str, data: &str) -> SseEvent {
	(name.to_string(), data.to_string())
}

/// The SSE body of `createSseResponse`: `event: <name>\ndata: <data>\n` per event, joined with `\n`.
pub fn sse_body(events: &[SseEvent]) -> String {
	events
		.iter()
		.map(|(name, data)| format!("event: {name}\ndata: {data}\n"))
		.collect::<Vec<_>>()
		.join("\n")
}

/// `createSseResponse(events)`.
pub fn sse_response(events: &[SseEvent]) -> HttpResponse {
	let headers = IndexMap::from([("content-type".to_string(), "text/event-stream".to_string())]);
	HttpResponse::from_bytes(200, headers, sse_body(events))
}

/// `minimalAnthropicEvents`: one text block saying "Hello".
pub fn minimal_events() -> Vec<SseEvent> {
	vec![
		event(
			"message_start",
			json!({
				"type": "message_start",
				"message": {
					"id": "msg_test",
					"usage": { "input_tokens": 12, "output_tokens": 0, "cache_read_input_tokens": 0, "cache_creation_input_tokens": 0 },
				},
			}),
		),
		event(
			"content_block_start",
			json!({ "type": "content_block_start", "index": 0, "content_block": { "type": "text", "text": "" } }),
		),
		event(
			"content_block_delta",
			json!({ "type": "content_block_delta", "index": 0, "delta": { "type": "text_delta", "text": "Hello" } }),
		),
		event(
			"content_block_stop",
			json!({ "type": "content_block_stop", "index": 0 }),
		),
		event(
			"message_delta",
			json!({
				"type": "message_delta",
				"delta": { "stop_reason": "end_turn" },
				"usage": { "input_tokens": 12, "output_tokens": 5, "cache_read_input_tokens": 0, "cache_creation_input_tokens": 0 },
			}),
		),
		event("message_stop", json!({ "type": "message_stop" })),
	]
}

type Respond = Box<dyn Fn(&Value, &RequestOptions) -> PiResult<HttpResponse>>;

/// A fake SDK client: answers each `beta.messages.create` call with `respond` and records its params.
pub struct FakeClient {
	respond: Respond,
	pub params: RefCell<Vec<Value>>,
	pub options: RefCell<Vec<RequestOptions>>,
}

impl FakeClient {
	/// A client answering every call with `respond(params, options)`.
	pub fn with(respond: impl Fn(&Value, &RequestOptions) -> PiResult<HttpResponse> + 'static) -> Rc<Self> {
		Rc::new(Self {
			respond: Box::new(respond),
			params: RefCell::new(Vec::new()),
			options: RefCell::new(Vec::new()),
		})
	}

	/// `createFakeAnthropicClient(response)`: one response; a second call fails like a consumed body.
	pub fn new(response: HttpResponse) -> Rc<Self> {
		let response = RefCell::new(Some(response));
		Self::with(move |_, _| {
			response
				.borrow_mut()
				.take()
				.ok_or_else(|| PiError::type_error("Body is unusable: Body has already been read"))
		})
	}

	/// The params of the last call.
	pub fn last_params(&self) -> Value {
		self.params.borrow().last().cloned().expect("the client was called")
	}
}

impl AnthropicClientApi for FakeClient {
	fn beta_messages_create<'a>(
		&'a self,
		params: &'a Value,
		options: RequestOptions,
	) -> LocalBoxFuture<'a, PiResult<HttpResponse>> {
		self.params.borrow_mut().push(params.clone());
		let response = (self.respond)(params, &options);
		self.options.borrow_mut().push(options);
		Box::pin(async move { response })
	}
}

/// `getModel(provider, id)`.
pub fn model(provider: &str, id: &str) -> Model {
	get_builtin_model(provider, id).unwrap_or_else(|| panic!("no built-in model {provider}/{id}"))
}

/// `normalizeContext(context)` of a TS context literal.
pub fn context(context: Value) -> TranscriptContext {
	let context: Context = serde_json::from_value(context).expect("valid context");
	normalize_context(&context)
}

/// `normalizeContext({ messages: [{ role: "user", content: text, timestamp }] })`.
pub fn user_context(text: &str) -> TranscriptContext {
	context(json!({ "messages": [{ "role": "user", "content": text, "timestamp": 1 }] }))
}
