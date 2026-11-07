//! Shared helpers for the OpenAI Responses test ports: models, contexts, canned fetch responses and payload capture.

#![allow(dead_code)]

use std::cell::RefCell;
use std::future::Future;
use std::pin::Pin;
use std::rc::Rc;
use std::task::{Context as TaskContext, Poll};

use futures::StreamExt;

use indexmap::IndexMap;
use pi_ai::api::openai_responses::OpenAIResponsesOptions;
use pi_ai::types::{
	AssistantMessage, Context, Model, OnPayload, SharedAssistantMessage, StopReason, TranscriptContext, Usage,
};
use pi_ai::utils::transcript::normalize_context;
use pi_host::{FetchFunction, HttpResponse, PiResult};
use serde_json::{Value, json};

/// A `Model` from its TS object literal.
pub fn model(value: Value) -> Model {
	serde_json::from_value(value).expect("model JSON is a Model")
}

/// The `gpt-5-mini` openai-responses model most of the TS files declare.
pub fn gpt5_mini() -> Model {
	model(json!({
		"id": "gpt-5-mini",
		"name": "GPT-5 Mini",
		"api": "openai-responses",
		"provider": "openai",
		"baseUrl": "https://api.openai.com/v1",
		"reasoning": true,
		"input": ["text"],
		"cost": { "input": 0, "output": 0, "cacheRead": 0, "cacheWrite": 0 },
		"contextWindow": 400000,
		"maxTokens": 128000,
	}))
}

/// `model` with the object `patch` spread over it: `{ ...model, ...patch }`.
pub fn with(model: &Model, patch: Value) -> Model {
	let mut value = serde_json::to_value(model).expect("model serializes");
	let (Value::Object(target), Value::Object(patch)) = (&mut value, patch) else {
		panic!("model and patch are objects");
	};
	for (key, entry) in patch {
		target.insert(key, entry);
	}
	serde_json::from_value(value).expect("patched model is a Model")
}

/// `normalizeContext(context)` of a TS context literal.
pub fn context(value: Value) -> TranscriptContext {
	normalize_context(&serde_json::from_value::<Context>(value).expect("context JSON is a Context"))
}

/// The all-zero `Usage` literal of the TS files.
pub fn usage() -> Value {
	json!({
		"input": 0,
		"output": 0,
		"cacheRead": 0,
		"cacheWrite": 0,
		"totalTokens": 0,
		"cost": { "input": 0, "output": 0, "cacheRead": 0, "cacheWrite": 0, "total": 0 },
	})
}

/// `new Response(body, { status, headers })`.
pub fn response(status: u16, headers: &[(&str, &str)], body: impl Into<String>) -> HttpResponse {
	let headers: IndexMap<String, String> = headers
		.iter()
		.map(|(name, value)| ((*name).to_string(), (*value).to_string()))
		.collect();
	HttpResponse::from_bytes(status, headers, body.into())
}

/// A JSON error response like `new Response(JSON.stringify(body), { status, headers: { "content-type": ... } })`.
pub fn json_response(status: u16, body: &Value) -> HttpResponse {
	response(
		status,
		&[("content-type", "application/json")],
		pi_host::js::json_stringify(body),
	)
}

/// An SSE response whose events are `event: <type>\ndata: <json>\n\n`.
pub fn sse_response(events: &[Value]) -> HttpResponse {
	response(200, &[("content-type", "text/event-stream")], sse_body(events))
}

/// The SSE body of `events`, each tagged with its `type`.
pub fn sse_body(events: &[Value]) -> String {
	events
		.iter()
		.map(|event| {
			format!(
				"event: {}\ndata: {}\n\n",
				event["type"].as_str().unwrap_or_default(),
				pi_host::js::json_stringify(event)
			)
		})
		.collect()
}

/// `fetch: async () => make()`.
pub fn fetch_returning(make: impl Fn() -> HttpResponse + 'static) -> FetchFunction {
	Rc::new(move |_| {
		let response = make();
		Box::pin(async move { Ok(response) })
	})
}

/// `onPayload: (params) => { payload = params }`, with the captured payload.
pub fn capture_payload() -> (OnPayload, Rc<RefCell<Option<Value>>>) {
	let captured = Rc::new(RefCell::new(None));
	let target = captured.clone();
	let on_payload: OnPayload = Rc::new(move |params, _model| {
		*target.borrow_mut() = Some(params);
		Box::pin(async { Ok(None) })
	});
	(on_payload, captured)
}

/// `createOutput()`: an empty pending assistant message for `model`.
pub fn create_output(model: &Model) -> SharedAssistantMessage {
	Rc::new(RefCell::new(AssistantMessage {
		content: Vec::new(),
		api: model.api.clone(),
		provider: model.provider.clone(),
		model: model.id.clone(),
		usage: Usage::default(),
		stop_reason: StopReason::Pending,
		timestamp: pi_host::now_ms(),
		..AssistantMessage::default()
	}))
}

/// An `AsyncIterable<ResponseStreamEvent>` yielding `events`.
pub fn events(events: Vec<Value>) -> impl futures::Stream<Item = PiResult<Value>> + Unpin {
	futures::stream::iter(events.into_iter().map(Ok))
}

/// `expect(actual).toMatchObject(expected)` for JSON values; numbers compare as `f64`, like JS numbers.
pub fn assert_match_object(actual: &Value, expected: &Value) {
	match (actual, expected) {
		(Value::Object(actual_map), Value::Object(expected_map)) => {
			for (key, expected) in expected_map {
				let actual_value = actual_map
					.get(key)
					.unwrap_or_else(|| panic!("missing key {key} in {actual}"));
				assert_match_object(actual_value, expected);
			}
		}
		(Value::Array(actual_items), Value::Array(expected_items)) => {
			assert_eq!(actual_items.len(), expected_items.len(), "{actual} vs {expected}");
			for (actual, expected) in actual_items.iter().zip(expected_items) {
				assert_match_object(actual, expected);
			}
		}
		(Value::Number(actual_number), Value::Number(expected_number)) => {
			assert_eq!(actual_number.as_f64(), expected_number.as_f64());
		}
		_ => assert_eq!(actual, expected),
	}
}

/// Resolves on its second poll, so other tasks run in between, like an `await` of a resolved promise.
pub struct YieldNow(bool);

impl Future for YieldNow {
	type Output = ();

	fn poll(mut self: Pin<&mut Self>, cx: &mut TaskContext<'_>) -> Poll<()> {
		if self.0 {
			return Poll::Ready(());
		}
		self.0 = true;
		cx.waker().wake_by_ref();
		Poll::Pending
	}
}

/// Lets spawned tasks run before continuing.
pub fn yield_now() -> YieldNow {
	YieldNow(false)
}

/// Like `events`, but yields to other tasks before each event, so a concurrent consumer of the event stream sees
/// each pushed event before the next provider event is processed (what a synchronous `stream.push` spy observes).
pub fn yielding_events(events: Vec<Value>) -> impl futures::Stream<Item = PiResult<Value>> + Unpin {
	Box::pin(futures::stream::iter(events).then(|event| async move {
		yield_now().await;
		Ok(event)
	}))
}

/// `response` whose body arrives on a later timer tick, like a network body: consumers of the event stream see the
/// events pushed before the body is read.
pub fn delayed_body(mut response: HttpResponse) -> HttpResponse {
	let body = std::mem::replace(&mut response.body, Box::pin(futures::stream::empty()));
	response.body = Box::pin(
		futures::stream::once(pi_host::sleep(0.0, None))
			.filter_map(|_| async { None })
			.chain(body),
	);
	response
}

/// An `OpenAIResponsesOptions` literal: the defaults with `configure` applied.
pub fn responses_options(configure: impl FnOnce(&mut OpenAIResponsesOptions)) -> OpenAIResponsesOptions {
	let mut options = OpenAIResponsesOptions::default();
	configure(&mut options);
	options
}
