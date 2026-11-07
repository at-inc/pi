//! Port of `packages/ai/test/bedrock-raw-stop-reason.test.ts`.
//!
//! Differences from the TS:
//! - The TS mocks `@aws-sdk/client-bedrock-runtime` so `client.send()` resolves with a stream of the given items. The
//!   port serves the same items as event stream frames and lets `api::bedrock_client` decode them. Credentials come
//!   from `AWS_ACCESS_KEY_ID`/`AWS_SECRET_ACCESS_KEY` in the process env, which the mocked client did not need.
//! - "forwards SDK stream items in order before normalizing them": `expect(item).toBe(streamEvents[index])` checks
//!   object identity with the mock's items, which the HTTP level cannot show (the client decodes new values); the
//!   port keeps the equality checks. The models passed to the callback are compared as JSON.
//! - "forwards SDK error items before reporting them": the mock yields `{ internalServerException: new Error(...) }`
//!   as an item. The real SDK never yields an exception member: the event stream unmarshaller throws an exception frame
//!   as the named service exception. The port sends an `internalServerException` frame, so the callback receives only
//!   the `messageStart` item and the message carries the SDK exception's prefix:
//!   `Internal server error: bedrock stream failed`.

mod bedrock_support;

use std::cell::RefCell;
use std::rc::Rc;

use bedrock_support::{builtin_model, collect, event, exception, serve, stream_response};
use pi_ai::api::bedrock_converse_stream::{BedrockOptions, stream};
use pi_ai::types::{AssistantMessage, CacheRetention, Model, StopReason, StreamOptions, TranscriptContext};
use pi_host::testing::{TestHost, run};
use serde_json::{Value, json};

fn model() -> Model {
	builtin_model("us.anthropic.claude-opus-4-8")
}

fn context() -> TranscriptContext {
	bedrock_support::context(json!({ "messages": [{ "role": "user", "content": "hello", "timestamp": 1 }] }))
}

/// Static AWS credentials in the process env, which the default chain picks up.
fn set_credentials(host: &TestHost) {
	host.set_env("AWS_ACCESS_KEY_ID", "AKIDEXAMPLE");
	host.set_env("AWS_SECRET_ACCESS_KEY", "wJalrXUtnFEMI/K7MDENG+bPxRfiCYEXAMPLEKEY");
}

/// `{ cacheRetention: "none" }`.
fn options() -> BedrockOptions {
	BedrockOptions {
		base: StreamOptions {
			cache_retention: Some(CacheRetention::None),
			..StreamOptions::default()
		},
		..BedrockOptions::default()
	}
}

/// The mock's default stream: `messageStart`, then `messageStop` with `stopReason`.
fn stop_stream(host: &TestHost, stop_reason: &str) {
	serve(
		host,
		vec![stream_response(
			&[
				event("messageStart", json!({ "role": "assistant" })),
				event("messageStop", json!({ "stopReason": stop_reason })),
			],
			&[("x-amzn-requestid", "request-id")],
		)],
	);
}

type Received = Rc<RefCell<Vec<(Value, Value)>>>;

/// Streams with an `onProviderStreamEvent` that records each item and the model it was given.
async fn stream_recording(options: BedrockOptions) -> (Vec<(Value, Value)>, AssistantMessage) {
	let received: Received = Rc::default();
	let recorder = received.clone();
	let mut options = options;
	options.base.on_provider_stream_event = Some(Rc::new(move |item: &Value, event_model: &Model| {
		recorder
			.borrow_mut()
			.push((item.clone(), serde_json::to_value(event_model).unwrap()));
		Box::pin(async { Ok(()) })
	}));
	let (_, message) = collect(stream(&model(), &context(), Some(options))).await;
	(received.take(), message)
}

mod bedrock_provider_stream_events {
	use super::*;

	#[test]
	fn forwards_sdk_stream_items_in_order_before_normalizing_them() {
		run(|host| async move {
			set_credentials(&host);
			let stream_events = vec![
				json!({ "messageStart": { "role": "assistant" } }),
				json!({ "contentBlockDelta": { "contentBlockIndex": 0, "delta": { "text": "hello" } } }),
				json!({ "messageStop": { "stopReason": "end_turn", "additionalModelResponseFields": { "source": "test" } } }),
				json!({ "metadata": { "usage": { "inputTokens": 1, "outputTokens": 1, "totalTokens": 2 } } }),
			];
			let frames: Vec<Vec<u8>> = stream_events
				.iter()
				.map(|item| {
					let (member, value) = item.as_object().unwrap().iter().next().unwrap();
					event(member, value.clone())
				})
				.collect();
			serve(&host, vec![stream_response(&frames, &[])]);

			let (received, result) = stream_recording(options()).await;

			let items: Vec<Value> = received.iter().map(|(item, _)| item.clone()).collect();
			assert_eq!(items, stream_events);
			let event_models: Vec<Value> = received.iter().map(|(_, event_model)| event_model.clone()).collect();
			let model = serde_json::to_value(model()).unwrap();
			assert_eq!(event_models, [model.clone(), model.clone(), model.clone(), model]);
			assert_eq!(result.stop_reason, StopReason::Stop);
			assert_eq!(
				serde_json::to_value(&result.content).unwrap(),
				json!([{ "type": "text", "text": "hello" }])
			);
		});
	}

	#[test]
	fn forwards_sdk_error_items_before_reporting_them() {
		run(|host| async move {
			set_credentials(&host);
			serve(
				&host,
				vec![stream_response(
					&[
						event("messageStart", json!({ "role": "assistant" })),
						exception("internalServerException", json!({ "message": "bedrock stream failed" })),
					],
					&[],
				)],
			);

			let (received, result) = stream_recording(options()).await;

			let items: Vec<Value> = received.into_iter().map(|(item, _)| item).collect();
			assert_eq!(items, [json!({ "messageStart": { "role": "assistant" } })]);
			assert_eq!(result.stop_reason, StopReason::Error);
			assert_eq!(
				result.error_message.as_deref(),
				Some("Internal server error: bedrock stream failed")
			);
		});
	}
}

mod bedrock_raw_stop_reasons {
	use super::*;

	#[test]
	fn preserves_raw_bedrock_stop_reasons_for_successful_stops() {
		run(|host| async move {
			set_credentials(&host);
			stop_stream(&host, "end_turn");

			let (_, message) = collect(stream(&model(), &context(), Some(options()))).await;

			assert_eq!(message.stop_reason, StopReason::Stop);
			assert_eq!(message.raw_stop_reason.as_deref(), Some("end_turn"));
			assert_eq!(message.error_message, None);
		});
	}

	#[test]
	fn preserves_raw_bedrock_stop_reasons_for_provider_error_stops() {
		run(|host| async move {
			set_credentials(&host);
			stop_stream(&host, "guardrail_intervened");

			let (_, message) = collect(stream(&model(), &context(), Some(options()))).await;

			assert_eq!(message.stop_reason, StopReason::Error);
			assert_eq!(message.raw_stop_reason.as_deref(), Some("guardrail_intervened"));
			assert_eq!(
				message.error_message.as_deref(),
				Some("Provider stopped with: guardrail_intervened")
			);
		});
	}
}
