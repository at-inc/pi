//! Port of `packages/ai/test/anthropic-thinking-disable.test.ts`.
//!
//! Not ported: the "Anthropic thinking disable E2E" describe ("disables thinking for Claude reasoning models"), an e2e
//! case that needs a real `ANTHROPIC_API_KEY` and network access.

mod anthropic_support;

use std::cell::RefCell;
use std::rc::Rc;

use anthropic_support::model;
use pi_ai::compat::stream_simple;
use pi_ai::types::{Context, Model, SimpleStreamOptions, ThinkingLevel};
use pi_host::PiError;
use pi_host::testing::run;
use serde_json::{Value, json};

/// `makePayloadCaptureContext()`.
fn make_context() -> Context {
	serde_json::from_value(json!({ "messages": [{ "role": "user", "content": "Hello", "timestamp": 1 }] })).unwrap()
}

/// `capturePayload(model, options)`: the model with `baseUrl: "http://127.0.0.1:9"`.
async fn capture(model: &Model, options: SimpleStreamOptions) -> Value {
	let mut payload_capture_model = model.clone();
	payload_capture_model.base_url = "http://127.0.0.1:9".into();
	capture_payload(&payload_capture_model, &make_context(), options).await
}

fn reasoning(level: ThinkingLevel) -> SimpleStreamOptions {
	SimpleStreamOptions {
		reasoning: Some(level),
		..SimpleStreamOptions::default()
	}
}

/// `capturePayload(model, context)` through the compat `streamSimple`: `onPayload` records the payload and throws.
async fn capture_payload(model: &Model, context: &Context, options: SimpleStreamOptions) -> Value {
	let captured: Rc<RefCell<Option<Value>>> = Rc::default();
	let recorder = captured.clone();
	let mut options = options;
	options.api_key = Some("fake-key".into());
	options.on_payload = Some(Rc::new(move |payload, _| {
		*recorder.borrow_mut() = Some(payload);
		Box::pin(async { Err(PiError::named("PayloadCaptured", "payload captured")) })
	}));
	stream_simple(model, context, Some(options)).unwrap().result().await;
	captured
		.borrow_mut()
		.take()
		.expect("Expected payload capture before request")
}

mod anthropic_thinking_disable_payload {
	use super::*;

	#[test]
	fn sends_thinking_type_disabled_for_budget_based_reasoning_models_when_thinking_is_off() {
		run(|_| async {
			let payload = capture(&model("anthropic", "claude-sonnet-4-5"), SimpleStreamOptions::default()).await;

			assert_eq!(payload.get("thinking"), Some(&json!({ "type": "disabled" })));
			assert_eq!(payload.get("output_config"), None);
		});
	}

	#[test]
	fn sends_thinking_type_disabled_for_adaptive_reasoning_models_when_thinking_is_off() {
		run(|_| async {
			let payload = capture(&model("anthropic", "claude-opus-4-6"), SimpleStreamOptions::default()).await;

			assert_eq!(payload.get("thinking"), Some(&json!({ "type": "disabled" })));
			assert_eq!(payload.get("output_config"), None);
		});
	}

	#[test]
	fn sends_thinking_type_disabled_for_claude_opus_4_8_when_thinking_is_off() {
		run(|_| async {
			let payload = capture(&model("anthropic", "claude-opus-4-8"), SimpleStreamOptions::default()).await;

			assert_eq!(payload.get("thinking"), Some(&json!({ "type": "disabled" })));
			assert_eq!(payload.get("output_config"), None);
		});
	}

	#[test]
	fn omits_thinking_type_disabled_for_claude_fable_5_when_thinking_is_off() {
		run(|_| async {
			let payload = capture(&model("anthropic", "claude-fable-5"), SimpleStreamOptions::default()).await;

			assert_eq!(payload.get("thinking"), None);
			assert_eq!(payload.get("output_config"), None);
		});
	}

	#[test]
	fn uses_adaptive_thinking_for_claude_opus_4_8_when_reasoning_is_enabled() {
		run(|_| async {
			let payload = capture(&model("anthropic", "claude-opus-4-8"), reasoning(ThinkingLevel::High)).await;

			assert_eq!(
				payload.get("thinking"),
				Some(&json!({ "type": "adaptive", "display": "summarized" }))
			);
			assert_eq!(payload.get("output_config"), Some(&json!({ "effort": "high" })));
		});
	}

	#[test]
	fn uses_adaptive_thinking_for_claude_sonnet_5_when_reasoning_is_enabled() {
		run(|_| async {
			let payload = capture(&model("anthropic", "claude-sonnet-5"), reasoning(ThinkingLevel::High)).await;

			assert_eq!(
				payload.get("thinking"),
				Some(&json!({ "type": "adaptive", "display": "summarized" }))
			);
			assert_eq!(payload.get("output_config"), Some(&json!({ "effort": "high" })));
		});
	}

	#[test]
	fn maps_xhigh_reasoning_to_effort_xhigh_for_claude_opus_4_8() {
		run(|_| async {
			let payload = capture(&model("anthropic", "claude-opus-4-8"), reasoning(ThinkingLevel::Xhigh)).await;

			assert_eq!(
				payload.get("thinking"),
				Some(&json!({ "type": "adaptive", "display": "summarized" }))
			);
			assert_eq!(payload.get("output_config"), Some(&json!({ "effort": "xhigh" })));
		});
	}
}
