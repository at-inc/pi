//! Shared helpers for the Bedrock tests: a Bedrock model, event stream frames, and a fake Bedrock Runtime endpoint on
//! the test host. The TS tests `vi.mock` `@aws-sdk/client-bedrock-runtime`; the Rust tests answer at the HTTP level
//! and read what the client sent from the recorded requests.

#![allow(dead_code)]

use std::cell::Cell;

use aws_smithy_eventstream::frame::write_message_to;
use aws_smithy_types::event_stream::{Header, HeaderValue, Message};
use indexmap::IndexMap;
use pi_ai::types::{AssistantMessage, AssistantMessageEvent, Context, Model, TranscriptContext};
use pi_ai::utils::event_stream::AssistantMessageEventStream;
use pi_ai::utils::transcript::normalize_context;
use pi_host::HttpResponse;
use pi_host::testing::{RecordedRequest, TestHost};
use serde_json::{Value, json};

/// A Bedrock model from its TS literal.
pub fn model(model: Value) -> Model {
	serde_json::from_value(model).expect("a valid model")
}

/// `getModel("amazon-bedrock", id)`.
pub fn builtin_model(id: &str) -> Model {
	pi_ai::providers::all::get_builtin_model("amazon-bedrock", id).expect("a built-in Bedrock model")
}

/// `normalizeContext(context)` of a TS context literal.
pub fn context(context: Value) -> TranscriptContext {
	normalize_context(&serde_json::from_value::<Context>(context).expect("a valid context"))
}

/// One event stream message: an `event` of `event_type` with a JSON payload.
pub fn event(event_type: &str, payload: Value) -> Vec<u8> {
	frame("event", event_type, payload.to_string().as_bytes(), &[])
}

/// One `exception` message of `exception_type` with a JSON payload.
pub fn exception(exception_type: &str, payload: Value) -> Vec<u8> {
	frame("exception", exception_type, payload.to_string().as_bytes(), &[])
}

/// One `error` message with `:error-code` and `:error-message`.
pub fn error_message(code: &str, message: &str) -> Vec<u8> {
	frame("error", "", b"", &[(":error-code", code), (":error-message", message)])
}

/// An event stream message like the oracle's `frame(eventType, payload, messageType, extraHeaders)`.
pub fn frame(message_type: &str, event_type: &str, payload: &[u8], extra_headers: &[(&str, &str)]) -> Vec<u8> {
	let mut message = Message::new(payload.to_vec()).add_header(Header::new(
		":message-type",
		HeaderValue::String(message_type.to_string().into()),
	));
	match message_type {
		"event" => {
			message = message.add_header(Header::new(
				":event-type",
				HeaderValue::String(event_type.to_string().into()),
			))
		}
		"exception" => {
			message = message.add_header(Header::new(
				":exception-type",
				HeaderValue::String(event_type.to_string().into()),
			))
		}
		_ => {}
	}
	message = message.add_header(Header::new(
		":content-type",
		HeaderValue::String("application/json".into()),
	));
	for (name, value) in extra_headers {
		message = message.add_header(Header::new(
			name.to_string(),
			HeaderValue::String(value.to_string().into()),
		));
	}
	let mut bytes = Vec::new();
	write_message_to(&message, &mut bytes).expect("an encodable message");
	bytes
}

/// A canned HTTP response, replayable for every attempt.
#[derive(Clone)]
pub struct Reply {
	pub status: u16,
	pub headers: IndexMap<String, String>,
	pub body: Vec<u8>,
}

impl Reply {
	pub fn to_response(&self) -> HttpResponse {
		HttpResponse::from_bytes(self.status, self.headers.clone(), self.body.clone())
	}
}

/// A 200 event stream response with the given messages and extra headers.
pub fn stream_response(messages: &[Vec<u8>], headers: &[(&str, &str)]) -> Reply {
	let mut reply = reply(200, headers, messages.concat());
	reply
		.headers
		.shift_insert(0, "content-type".into(), "application/vnd.amazon.eventstream".into());
	reply
}

/// A response with a status, headers and a text body.
pub fn json_response(status: u16, headers: &[(&str, &str)], body: &str) -> Reply {
	reply(status, headers, body.as_bytes().to_vec())
}

fn reply(status: u16, headers: &[(&str, &str)], body: Vec<u8>) -> Reply {
	Reply {
		status,
		headers: headers
			.iter()
			.map(|(name, value)| (name.to_string(), value.to_string()))
			.collect(),
		body,
	}
}

/// The events of a minimal successful text response.
pub fn text_stream(text: &str) -> Vec<Vec<u8>> {
	vec![
		event("messageStart", json!({ "role": "assistant" })),
		event(
			"contentBlockDelta",
			json!({ "contentBlockIndex": 0, "delta": { "text": text } }),
		),
		event("contentBlockStop", json!({ "contentBlockIndex": 0 })),
		event("messageStop", json!({ "stopReason": "end_turn" })),
		event(
			"metadata",
			json!({ "usage": { "inputTokens": 1, "outputTokens": 1, "totalTokens": 2 } }),
		),
	]
}

/// Answers every request with the next response of `responses` (the last one repeats).
pub fn serve(host: &TestHost, responses: Vec<Reply>) {
	let next = Cell::new(0usize);
	host.on_fetch(move |_| {
		let index = next.get().min(responses.len() - 1);
		next.set(next.get() + 1);
		let response = responses[index].to_response();
		Box::pin(async move { Ok(response) })
	});
}

/// The parsed JSON body of a recorded `ConverseStream` request.
pub fn body(request: &RecordedRequest) -> Value {
	request.json()
}

/// Collects the event types of a stream and its final message.
pub async fn collect(stream: AssistantMessageEventStream) -> (Vec<&'static str>, AssistantMessage) {
	let mut types = Vec::new();
	while let Some(event) = stream.next().await {
		types.push(event_type(&event));
	}
	let message = stream.result().await.expect("a final message");
	(types, message)
}

/// The TS `event.type`.
pub fn event_type(event: &AssistantMessageEvent) -> &'static str {
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
