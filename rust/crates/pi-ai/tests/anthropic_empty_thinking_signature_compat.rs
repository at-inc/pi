//! Port of `packages/ai/test/anthropic-empty-thinking-signature-compat.test.ts`.
//!
//! Differences from the TS: `it.each` cases run in a loop. `model.compat?.allowEmptySignature` reads the model's raw
//! `compat` object.

mod anthropic_support;

use std::cell::RefCell;
use std::rc::Rc;

use anthropic_support::model;
use pi_ai::compat::stream_simple;
use pi_ai::providers::all::get_builtin_models;
use pi_ai::types::{Context, Model, SimpleStreamOptions};
use pi_host::PiError;
use pi_host::testing::run;
use serde_json::{Map, Value, json};

/// `makeModel(allowEmptySignature)`.
fn make_model(allow_empty_signature: Option<bool>) -> Model {
	let mut model = json!({
		"id": "mimo-v2.6-pro",
		"name": "MiMo-V2.5-Pro",
		"api": "anthropic-messages",
		"provider": "xiaomi-token-plan-ams",
		"baseUrl": "http://127.0.0.1:9/anthropic",
		"reasoning": true,
		"input": ["text"],
		"cost": { "input": 0, "output": 0, "cacheRead": 0, "cacheWrite": 0 },
		"contextWindow": 1048576,
		"maxTokens": 1024,
	});
	if let Some(allow_empty_signature) = allow_empty_signature {
		model["compat"] = json!({ "allowEmptySignature": allow_empty_signature });
	}
	serde_json::from_value(model).unwrap()
}

/// `makeContext(thinkingSignature, thinking, provider, model)` as its JSON literal, so callers can extend the
/// assistant content before deserializing it.
fn make_context_value(thinking_signature: &str, thinking: &str, provider: &str, model: &str) -> Value {
	let assistant = json!({
		"role": "assistant",
		"content": [{ "type": "thinking", "thinking": thinking, "thinkingSignature": thinking_signature }],
		"provider": provider,
		"api": "anthropic-messages",
		"model": model,
		"timestamp": 1,
		"usage": {
			"input": 0,
			"output": 0,
			"cacheRead": 0,
			"cacheWrite": 0,
			"totalTokens": 0,
			"cost": { "input": 0, "output": 0, "cacheRead": 0, "cacheWrite": 0, "total": 0 },
		},
		"stopReason": "stop",
	});
	json!({
		"messages": [
			{ "role": "user", "content": "first", "timestamp": 1 },
			assistant,
			{ "role": "user", "content": "second", "timestamp": 1 },
		],
	})
}

/// `makeContext(thinkingSignature, thinking)` with the default provider and model.
fn make_context(thinking_signature: &str, thinking: &str) -> Context {
	serde_json::from_value(make_context_value(
		thinking_signature,
		thinking,
		"xiaomi-token-plan-ams",
		"mimo-v2.6-pro",
	))
	.unwrap()
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

/// `payload.messages?.find((message) => message.role === "assistant")?.content`.
fn assistant_content(payload: &Value) -> Option<Value> {
	payload
		.get("messages")?
		.as_array()?
		.iter()
		.find(|message| message.get("role") == Some(&json!("assistant")))?
		.get("content")
		.cloned()
}

/// `model.compat?.allowEmptySignature`.
fn allow_empty_signature(model: &Model) -> Option<Value> {
	model
		.compat
		.as_ref()
		.and_then(|compat: &Map<String, Value>| compat.get("allowEmptySignature"))
		.cloned()
}

mod anthropic_empty_thinking_signature_compat {
	use super::*;

	#[test]
	fn converts_empty_signature_thinking_to_text_by_default() {
		run(|_| async {
			let payload = capture_payload(
				&make_model(None),
				&make_context("", "internal reasoning"),
				SimpleStreamOptions::default(),
			)
			.await;
			assert_eq!(
				assistant_content(&payload),
				Some(json!([{ "type": "text", "text": "internal reasoning" }]))
			);
		});
	}

	#[test]
	fn preserves_empty_thinking_text_when_the_signature_is_present() {
		run(|_| async {
			let payload = capture_payload(
				&make_model(None),
				&make_context("signed-thinking", ""),
				SimpleStreamOptions::default(),
			)
			.await;
			assert_eq!(
				assistant_content(&payload),
				Some(json!([{ "type": "thinking", "thinking": "", "signature": "signed-thinking" }]))
			);
		});
	}

	#[test]
	fn preserves_empty_signature_thinking_when_allow_empty_signature_is_enabled() {
		run(|_| async {
			let payload = capture_payload(
				&make_model(Some(true)),
				&make_context(" ", "internal reasoning"),
				SimpleStreamOptions::default(),
			)
			.await;
			assert_eq!(
				assistant_content(&payload),
				Some(json!([{ "type": "thinking", "thinking": "internal reasoning", "signature": "" }]))
			);
		});
	}

	// Regression for #9676: Vercel AI Gateway emits unsigned thinking for translated models.
	#[test]
	fn allows_empty_thinking_signatures_for_every_vercel_ai_gateway_model() {
		let models = get_builtin_models("vercel-ai-gateway");
		assert!(!models.is_empty());
		assert!(
			models
				.iter()
				.all(|model| allow_empty_signature(model) == Some(json!(true)))
		);
	}

	// Regression for #9323: Fireworks emits unsigned thinking that must survive replay.
	#[test]
	fn preserves_unsigned_thinking_for_fireworks() {
		for model_id in [
			"accounts/fireworks/models/deepseek-v4p1-flash",
			"accounts/fireworks/models/qwen3p8-max",
			"accounts/fireworks/models/qwen3p8-2p4t-a95b",
			"accounts/fireworks/models/nemotron-3-ultra-nvfp4",
		] {
			run(|_| async move {
				let model = model("fireworks", model_id);
				assert_eq!(allow_empty_signature(&model), Some(json!(true)), "{model_id}");
				let mut context = make_context_value("", "internal reasoning", "fireworks", model_id);
				context["messages"][1]["content"]
					.as_array_mut()
					.unwrap()
					.push(json!({ "type": "text", "text": "answer" }));
				let context: Context = serde_json::from_value(context).unwrap();
				let payload = capture_payload(&model, &context, SimpleStreamOptions::default()).await;
				assert_eq!(
					assistant_content(&payload),
					Some(json!([
						{ "type": "thinking", "thinking": "internal reasoning", "signature": "" },
						{ "type": "text", "text": "answer" },
					])),
					"{model_id}"
				);
			});
		}
	}
}
