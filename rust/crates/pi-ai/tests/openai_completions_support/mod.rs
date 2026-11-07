//! Shared helpers of the openai-completions test ports.
//!
//! The vitest files replace the `openai` SDK with `vi.mock`; the ports mock one level lower, at the HTTP request
//! the SDK replacement (`api/openai_client.rs`) sends through the test host: the request body is the `create(params)`
//! argument, and an SSE response replays the mocked chunks.

#![allow(dead_code)]

use std::cell::RefCell;
use std::rc::Rc;

use futures::StreamExt;
use indexmap::IndexMap;
use pi_ai::api::openai_completions::OpenAICompletionsOptions;
use pi_ai::types::{
	AssistantMessageEvent, BaseModel, Context, Message, Model, ModelCost, ModelInputModality, OnPayload,
	TranscriptContext, UserMessage, UserMessageContent, api,
};
use pi_ai::utils::event_stream::AssistantMessageEventStream;
use pi_ai::utils::transcript::normalize_context;
use pi_host::js::json_stringify;
use pi_host::testing::TestHost;
use pi_host::{HttpResponse, PiError};
use serde_json::Value;

/// An `openai-completions` model literal.
pub fn model(id: &str, provider: &str, base_url: &str) -> Model {
	Model {
		base: BaseModel {
			id: id.into(),
			name: id.into(),
			api: api::OPENAI_COMPLETIONS.into(),
			provider: provider.into(),
			base_url: base_url.into(),
			input: vec![ModelInputModality::Text],
			cost: ModelCost::default(),
			..BaseModel::default()
		},
		reasoning: false,
		context_window: 128_000.0,
		max_tokens: 4096.0,
		..Model::default()
	}
}

/// A model of the built-in catalog without its `compat`, served through `openai-completions`.
pub fn builtin_model(provider: &str, id: &str) -> Model {
	let mut model = pi_ai::providers::all::get_builtin_model(provider, id).expect("built-in model");
	model.compat = None;
	model.base.api = api::OPENAI_COMPLETIONS.into();
	model
}

/// The SSE body that replays `chunks`, then `[DONE]`.
pub fn sse_body(chunks: &[Value]) -> String {
	let mut body: String = chunks
		.iter()
		.map(|chunk| format!("data: {}\n\n", json_stringify(chunk)))
		.collect();
	body.push_str("data: [DONE]\n\n");
	body
}

pub fn sse_response(chunks: &[Value]) -> HttpResponse {
	HttpResponse::from_bytes(
		200,
		IndexMap::from([("content-type".to_string(), "text/event-stream".to_string())]),
		sse_body(chunks),
	)
}

/// Serves every request with an SSE stream of `chunks`.
pub fn serve_chunks(host: &TestHost, chunks: Vec<Value>) {
	host.on_fetch(move |_| {
		let response = sse_response(&chunks);
		Box::pin(async move { Ok(response) })
	});
}

/// A JSON error response.
pub fn error_response(status: u16, headers: &[(&str, &str)], body: &str) -> HttpResponse {
	let mut response_headers = IndexMap::from([("content-type".to_string(), "application/json".to_string())]);
	for (name, value) in headers {
		response_headers.insert((*name).to_string(), (*value).to_string());
	}
	HttpResponse::from_bytes(status, response_headers, body.to_string())
}

/// The JSON body of the last request: the `params` the mocked `create` received.
pub fn last_body(host: &TestHost) -> Value {
	host.requests().last().expect("a request was sent").json()
}

pub fn user_message(text: &str) -> Message {
	Message::User(UserMessage {
		content: UserMessageContent::Text(text.into()),
		timestamp: 0.0,
		..UserMessage::default()
	})
}

/// `normalizeContext({ messages })`.
pub fn context(messages: Vec<Message>) -> TranscriptContext {
	normalize_context(&Context {
		messages,
		..Context::default()
	})
}

/// `{ apiKey }`.
pub fn options(api_key: &str) -> OpenAICompletionsOptions {
	let mut options = OpenAICompletionsOptions::default();
	options.base.api_key = Some(api_key.into());
	options
}

/// An `onPayload` that records the payload and keeps it, or fails with `error`.
pub fn capture_payload(error: Option<&str>) -> (OnPayload, Rc<RefCell<Option<Value>>>) {
	let captured: Rc<RefCell<Option<Value>>> = Rc::default();
	let target = captured.clone();
	let error = error.map(str::to_string);
	let on_payload: OnPayload = Rc::new(move |payload, _model| {
		*target.borrow_mut() = Some(payload);
		let error = error.clone();
		Box::pin(async move {
			match error {
				Some(error) => Err(PiError::new(error)),
				None => Ok(None),
			}
		})
	});
	(on_payload, captured)
}

/// Every event of a stream.
pub async fn collect_events(stream: AssistantMessageEventStream) -> Vec<AssistantMessageEvent> {
	stream.into_stream().collect().await
}
