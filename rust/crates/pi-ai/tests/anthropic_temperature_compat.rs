//! Port of `packages/ai/test/anthropic-temperature-compat.test.ts`.
//!
//! Differences from the TS: `it.each` cases run in a loop, and `toBe(0)` compares the payload number as an `f64`, since
//! JSON `0` and `0.0` are the same JS number.

mod anthropic_support;

use std::cell::RefCell;
use std::rc::Rc;

use anthropic_support::model;
use pi_ai::compat::stream_simple;
use pi_ai::types::{Context, Model, ProviderHeaders, SimpleStreamOptions, StreamOptions};
use pi_host::PiError;
use pi_host::testing::run;
use serde_json::{Value, json};

/// `makeContext()`.
fn make_context() -> Context {
	serde_json::from_value(json!({ "messages": [{ "role": "user", "content": "Hello", "timestamp": 1 }] })).unwrap()
}

/// `makeCustomModel(compat)`.
fn make_custom_model(compat: Value) -> Model {
	serde_json::from_value(json!({
		"id": "vendor--claude-opus-4-7",
		"name": "Vendor Proxy Opus 4.7",
		"api": "anthropic-messages",
		"provider": "vendor-proxy",
		"baseUrl": "http://127.0.0.1:9",
		"reasoning": true,
		"input": ["text"],
		"cost": { "input": 0, "output": 0, "cacheRead": 0, "cacheWrite": 0 },
		"contextWindow": 200000,
		"maxTokens": 32000,
		"compat": compat,
	}))
	.unwrap()
}

/// `capturePayload(model, { temperature, headers })`: the model with `baseUrl: "http://127.0.0.1:9"`, and
/// `headers: { "anthropic-version": "2023-06-01", ...options?.headers }`.
async fn capture(model: &Model, temperature: f64, api_version: Option<&str>) -> Value {
	let mut payload_capture_model = model.clone();
	payload_capture_model.base_url = "http://127.0.0.1:9".into();
	let mut headers = ProviderHeaders::new();
	headers.insert("anthropic-version".into(), Some("2023-06-01".into()));
	if let Some(api_version) = api_version {
		headers.insert("anthropic-version".into(), Some(api_version.into()));
	}
	let mut options = SimpleStreamOptions {
		base: StreamOptions {
			temperature: Some(temperature),
			..StreamOptions::default()
		},
		..SimpleStreamOptions::default()
	};
	options.headers = Some(headers);
	capture_payload(&payload_capture_model, &make_context(), options).await
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

mod anthropic_temperature_compatibility {
	use super::*;

	#[test]
	fn omits_temperature_for_claude_opus_4_7() {
		run(|_| async {
			let payload = capture(&model("anthropic", "claude-opus-4-7"), 0.0, None).await;

			assert_eq!(payload.get("temperature"), None);
		});
	}

	#[test]
	fn omits_temperature_for_claude_opus_4_8() {
		run(|_| async {
			let payload = capture(&model("anthropic", "claude-opus-4-8"), 0.0, None).await;

			assert_eq!(payload.get("temperature"), None);
		});
	}

	#[test]
	fn omits_default_temperature_for_claude_opus_4_7() {
		run(|_| async {
			let payload = capture(&model("anthropic", "claude-opus-4-7"), 1.0, None).await;

			assert_eq!(payload.get("temperature"), None);
		});
	}

	#[test]
	fn keeps_temperature_for_claude_opus_4_6() {
		run(|_| async {
			let payload = capture(&model("anthropic", "claude-opus-4-6"), 0.0, Some("2023-06-01")).await;

			assert_eq!(payload.get("temperature").and_then(Value::as_f64), Some(0.0));
		});
	}

	#[test]
	fn keeps_temperature_for_claude_sonnet_4_6() {
		run(|_| async {
			let payload = capture(&model("anthropic", "claude-sonnet-4-6"), 0.0, Some("2023-06-01")).await;

			assert_eq!(payload.get("temperature").and_then(Value::as_f64), Some(0.0));
		});
	}

	#[test]
	fn omits_temperature_for_on_api_version_2026_10_21() {
		for id in ["claude-opus-4-6", "claude-sonnet-4-6"] {
			run(|_| async move {
				let payload = capture(&model("anthropic", id), 0.0, Some("2026-10-21")).await;

				assert_eq!(payload.get("temperature"), None, "{id}");
			});
		}
	}

	#[test]
	fn omits_temperature_for_custom_models_with_supports_temperature_disabled() {
		run(|_| async {
			let payload = capture(&make_custom_model(json!({ "supportsTemperature": false })), 0.0, None).await;

			assert_eq!(payload.get("temperature"), None);
		});
	}
}
