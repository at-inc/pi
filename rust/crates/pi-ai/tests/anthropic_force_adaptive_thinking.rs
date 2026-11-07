//! Port of `packages/ai/test/anthropic-force-adaptive-thinking.test.ts`.
//!
//! No differences from the TS beyond Rust syntax: payloads are captured through the compat `streamSimple`.

mod anthropic_support;

use std::cell::RefCell;
use std::rc::Rc;

use anthropic_support::model;
use pi_ai::compat::stream_simple;
use pi_ai::types::{Context, Model, SimpleStreamOptions, ThinkingLevel};
use pi_host::PiError;
use pi_host::testing::run;
use serde_json::{Value, json};

/// `makeCustomModel(compat)`.
fn make_custom_model(compat: Option<Value>) -> Model {
	let mut model = json!({
		// Id intentionally does not match any built-in adaptive substring. This
		// mirrors corporate proxy schemes such as `anthropic--claude-opus-latest`.
		"id": "vendor--claude-opus-latest",
		"name": "Vendor Proxy Opus Latest",
		"api": "anthropic-messages",
		"provider": "vendor-proxy",
		"baseUrl": "http://127.0.0.1:9",
		"reasoning": true,
		"input": ["text"],
		"cost": { "input": 0, "output": 0, "cacheRead": 0, "cacheWrite": 0 },
		"contextWindow": 200000,
		"maxTokens": 32000,
	});
	if let Some(compat) = compat {
		model["compat"] = compat;
	}
	serde_json::from_value(model).unwrap()
}

/// `makeContext()`.
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

mod anthropic_force_adaptive_thinking_compat_override {
	use super::*;

	#[test]
	fn sends_legacy_thinking_payload_for_custom_model_ids_by_default() {
		run(|_| async {
			let payload = capture(&make_custom_model(None), reasoning(ThinkingLevel::Medium)).await;

			assert_eq!(payload["thinking"]["type"], json!("enabled"));
			assert_eq!(payload.get("output_config"), None);
		});
	}

	#[test]
	fn sends_adaptive_thinking_payload_when_compat_force_adaptive_thinking_is_true() {
		run(|_| async {
			let payload = capture(
				&make_custom_model(Some(json!({ "forceAdaptiveThinking": true }))),
				reasoning(ThinkingLevel::Medium),
			)
			.await;

			assert_eq!(
				payload.get("thinking"),
				Some(&json!({ "type": "adaptive", "display": "summarized" }))
			);
			assert_eq!(payload.get("output_config"), Some(&json!({ "effort": "medium" })));
		});
	}

	#[test]
	fn uses_adaptive_thinking_with_native_xhigh_effort_for_claude_fable_5() {
		run(|_| async {
			let payload = capture(&model("anthropic", "claude-fable-5"), reasoning(ThinkingLevel::Xhigh)).await;

			assert_eq!(
				payload.get("thinking"),
				Some(&json!({ "type": "adaptive", "display": "summarized" }))
			);
			assert_eq!(payload.get("output_config"), Some(&json!({ "effort": "xhigh" })));
		});
	}

	#[test]
	fn allows_built_in_adaptive_models_to_opt_out_with_compat_force_adaptive_thinking_false() {
		run(|_| async {
			let mut model = model("anthropic", "claude-opus-4-8");
			model.compat = json!({ "forceAdaptiveThinking": false }).as_object().cloned();
			let payload = capture(&model, reasoning(ThinkingLevel::Medium)).await;

			assert_eq!(payload["thinking"]["type"], json!("enabled"));
			assert_eq!(payload.get("output_config"), None);
		});
	}

	#[test]
	fn preserves_thinking_type_disabled_when_reasoning_is_off_regardless_of_override() {
		run(|_| async {
			let payload = capture(
				&make_custom_model(Some(json!({ "forceAdaptiveThinking": true }))),
				SimpleStreamOptions::default(),
			)
			.await;

			assert_eq!(payload.get("thinking"), Some(&json!({ "type": "disabled" })));
			assert_eq!(payload.get("output_config"), None);
		});
	}
}
