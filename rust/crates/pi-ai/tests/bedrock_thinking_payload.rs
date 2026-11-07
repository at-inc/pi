//! Port of `packages/ai/test/bedrock-thinking-payload.test.ts`.
//!
//! Differences from the TS:
//! - `onPayload` receives the `ConverseStreamCommand` input as a `Value` and fails the stream with a
//!   `PayloadCaptured` error, like the TS throw, so no request is sent.
//! - `it.each` cases run in a loop inside one `#[test]` each.
//! - The adapter builds payload numbers as `f64` (TS `number`), so `budget_tokens: 16384` is `16384.0` in the `Value`;
//!   the serialized body is `16384` either way.
//!
//! Not ported: "Bedrock Claude max tokens E2E" ("uses the model maxTokens cap instead of Bedrock's 4096-token default
//! for adaptive Claude models"). It is an e2e case that needs real AWS credentials.

mod bedrock_support;

use std::cell::RefCell;
use std::rc::Rc;

use bedrock_support::{builtin_model, collect};
use pi_ai::api::bedrock_converse_stream::{BedrockOptions, stream};
use pi_ai::types::{Model, ThinkingLevel, TranscriptContext};
use pi_host::PiError;
use pi_host::testing::run;
use serde_json::{Value, json};

const THINKING_BINDING_CONTROLS_BETA: &str = "thinking-binding-controls-2026-08-01";

fn adaptive_with_binding() -> Value {
	json!({
		"type": "adaptive",
		"display": "summarized",
		"block_binding": { "prefix_mismatch_behavior": "drop_block" },
	})
}

/// `makeContext()`, normalized.
fn make_context() -> TranscriptContext {
	bedrock_support::context(json!({ "messages": [{ "role": "user", "content": "Hello", "timestamp": 1 }] }))
}

/// `{ ...getModel("amazon-bedrock", base), id, name }`.
fn model_with(base: &str, id: &str, name: &str) -> Model {
	let mut model = builtin_model(base);
	model.id = id.into();
	model.name = name.into();
	model
}

fn thinking_level(level: &str) -> ThinkingLevel {
	serde_json::from_value(json!(level)).expect("a thinking level")
}

/// Streams with an `onPayload` that captures the command input and throws `PayloadCaptured`.
async fn capture(model: &Model, context: &TranscriptContext, mut options: BedrockOptions) -> Value {
	let captured: Rc<RefCell<Option<Value>>> = Rc::default();
	let recorder = captured.clone();
	options.on_payload = Some(Rc::new(move |payload: Value, _model: &Model| {
		*recorder.borrow_mut() = Some(payload);
		Box::pin(async { Err(PiError::named("PayloadCaptured", "payload captured")) })
	}));
	collect(stream(model, context, Some(options))).await;
	captured
		.take()
		.expect("Expected Bedrock payload to be captured before request abort")
}

/// `capturePayload(model, options)`: `reasoning` defaults to `"high"`.
async fn capture_payload(model: &Model, options: Option<BedrockOptions>) -> Value {
	let mut options = options.unwrap_or_default();
	options.reasoning = Some(options.reasoning.unwrap_or_else(|| thinking_level("high")));
	capture(model, &make_context(), options).await
}

fn with_reasoning(level: &str) -> Option<BedrockOptions> {
	Some(BedrockOptions {
		reasoning: Some(thinking_level(level)),
		..BedrockOptions::default()
	})
}

/// `payload.additionalModelRequestFields?.[key]`, `Value::Null` when absent.
fn field(payload: &Value, key: &str) -> Value {
	payload
		.get("additionalModelRequestFields")
		.and_then(|fields| fields.get(key))
		.cloned()
		.unwrap_or(Value::Null)
}

mod bedrock_thinking_payload {
	use super::*;

	#[test]
	fn uses_adaptive_thinking_for_claude_opus_4_8_when_reasoning_is_enabled() {
		run(|_host| async move {
			let model = model_with(
				"global.anthropic.claude-opus-4-6-v1",
				"global.anthropic.claude-opus-4-8-v1",
				"Claude Opus 4.8 (Global)",
			);

			let payload = capture_payload(&model, None).await;

			assert_eq!(field(&payload, "thinking"), adaptive_with_binding());
			assert_eq!(field(&payload, "output_config"), json!({ "effort": "high" }));
			assert_eq!(
				field(&payload, "anthropic_beta"),
				json!([THINKING_BINDING_CONTROLS_BETA])
			);
		});
	}

	#[test]
	fn maps_xhigh_reasoning_to_effort_xhigh_for_claude_opus_4_8() {
		run(|_host| async move {
			let model = model_with(
				"global.anthropic.claude-opus-4-6-v1",
				"global.anthropic.claude-opus-4-8-v1",
				"Claude Opus 4.8 (Global)",
			);

			let payload = capture_payload(&model, with_reasoning("xhigh")).await;

			assert_eq!(field(&payload, "thinking"), adaptive_with_binding());
			assert_eq!(field(&payload, "output_config"), json!({ "effort": "xhigh" }));
			assert_eq!(
				field(&payload, "anthropic_beta"),
				json!([THINKING_BINDING_CONTROLS_BETA])
			);
		});
	}

	#[test]
	fn uses_adaptive_thinking_for_claude_fable_5_when_reasoning_is_enabled() {
		run(|_host| async move {
			let model = builtin_model("global.anthropic.claude-fable-5");

			let payload = capture_payload(&model, None).await;

			assert_eq!(field(&payload, "thinking"), adaptive_with_binding());
			assert_eq!(field(&payload, "output_config"), json!({ "effort": "high" }));
			assert_eq!(
				field(&payload, "anthropic_beta"),
				json!([THINKING_BINDING_CONTROLS_BETA])
			);
		});
	}

	#[test]
	fn uses_adaptive_thinking_for_claude_sonnet_5_when_reasoning_is_enabled() {
		run(|_host| async move {
			let model = builtin_model("global.anthropic.claude-sonnet-5");

			let payload = capture_payload(&model, None).await;

			assert_eq!(field(&payload, "thinking"), adaptive_with_binding());
			assert_eq!(field(&payload, "output_config"), json!({ "effort": "high" }));
			assert_eq!(
				field(&payload, "anthropic_beta"),
				json!([THINKING_BINDING_CONTROLS_BETA])
			);
		});
	}

	#[test]
	fn uses_adaptive_thinking_for_claude_opus_5_when_reasoning_is_enabled() {
		run(|_host| async move {
			let model = builtin_model("global.anthropic.claude-opus-5");

			let payload = capture_payload(&model, None).await;

			assert_eq!(field(&payload, "thinking"), adaptive_with_binding());
			assert_eq!(field(&payload, "output_config"), json!({ "effort": "high" }));
			assert_eq!(
				field(&payload, "anthropic_beta"),
				json!([THINKING_BINDING_CONTROLS_BETA])
			);
		});
	}

	#[test]
	fn maps_xhigh_reasoning_to_effort_xhigh_for_claude_opus_5() {
		run(|_host| async move {
			let model = builtin_model("global.anthropic.claude-opus-5");

			let payload = capture_payload(&model, with_reasoning("xhigh")).await;

			assert_eq!(field(&payload, "thinking"), adaptive_with_binding());
			assert_eq!(field(&payload, "output_config"), json!({ "effort": "xhigh" }));
			assert_eq!(
				field(&payload, "anthropic_beta"),
				json!([THINKING_BINDING_CONTROLS_BETA])
			);
		});
	}

	#[test]
	fn maps_xhigh_reasoning_to_effort_xhigh_for_claude_fable_5() {
		run(|_host| async move {
			let model = builtin_model("global.anthropic.claude-fable-5");

			let payload = capture_payload(&model, with_reasoning("xhigh")).await;

			assert_eq!(field(&payload, "thinking"), adaptive_with_binding());
			assert_eq!(field(&payload, "output_config"), json!({ "effort": "xhigh" }));
		});
	}

	// https://github.com/earendil-works/pi/issues/10324
	#[test]
	fn sends_block_binding_and_the_binding_beta_for_claude_opus_5_5() {
		run(|_host| async move {
			let model = builtin_model("global.anthropic.claude-opus-5-5");

			let payload = capture_payload(&model, None).await;

			assert_eq!(field(&payload, "thinking"), adaptive_with_binding());
			assert_eq!(
				field(&payload, "anthropic_beta"),
				json!([THINKING_BINDING_CONTROLS_BETA])
			);
		});
	}

	// Bedrock rejects block_binding on 4.6 models: "Extra inputs are not permitted" (#10324)
	#[test]
	fn omits_block_binding_for_model_id() {
		for model_id in [
			"global.anthropic.claude-opus-4-6-v1",
			"global.anthropic.claude-sonnet-4-6",
		] {
			run(|_host| async move {
				let model = builtin_model(model_id);

				let payload = capture_payload(&model, None).await;

				assert_eq!(
					field(&payload, "thinking"),
					json!({ "type": "adaptive", "display": "summarized" }),
					"{model_id}"
				);
				assert_eq!(field(&payload, "anthropic_beta"), Value::Null, "{model_id}");
			});
		}
	}

	#[test]
	fn omits_display_for_govcloud_model_ids_on_non_adaptive_claude_thinking() {
		run(|_host| async move {
			let model = model_with(
				"us.anthropic.claude-sonnet-4-5-20250929-v1:0",
				"us-gov.anthropic.claude-sonnet-4-5-20250929-v1:0",
				"Claude Sonnet 4.5 (GovCloud)",
			);

			let payload = capture_payload(&model, None).await;

			assert_eq!(
				field(&payload, "thinking"),
				json!({ "type": "enabled", "budget_tokens": 16384.0 })
			);
			assert_eq!(
				field(&payload, "anthropic_beta"),
				json!(["interleaved-thinking-2025-05-14"])
			);
		});
	}

	#[test]
	fn omits_display_for_govcloud_regions_on_adaptive_claude_thinking() {
		run(|_host| async move {
			let model = model_with(
				"global.anthropic.claude-opus-4-6-v1",
				"global.anthropic.claude-opus-4-8-v1",
				"Claude Opus 4.8 (Global)",
			);

			let payload = capture_payload(
				&model,
				Some(BedrockOptions {
					region: Some("us-gov-west-1".into()),
					..BedrockOptions::default()
				}),
			)
			.await;

			assert_eq!(field(&payload, "thinking"), json!({ "type": "adaptive" }));
			assert_eq!(field(&payload, "output_config"), json!({ "effort": "high" }));
			assert_eq!(field(&payload, "anthropic_beta"), Value::Null);
		});
	}
}

mod bedrock_openai_reasoning_payload {
	use super::*;

	// Regression for #9331: the configured thinking level never reached OpenAI models on Bedrock.
	#[test]
	fn sends_reasoning_as_reasoning_effort_for_gpt_6_and_gpt_5_6() {
		for (reasoning, effort) in [
			("minimal", "low"),
			("low", "low"),
			("medium", "medium"),
			("high", "high"),
			("xhigh", "xhigh"),
			("max", "max"),
		] {
			for id in [
				"global.openai.gpt-6-sol",
				"us.openai.gpt-6-luna",
				"global.openai.gpt-5.6-sol",
			] {
				run(|_host| async move {
					let payload = capture_payload(&builtin_model(id), with_reasoning(reasoning)).await;

					assert_eq!(
						payload["additionalModelRequestFields"],
						json!({ "reasoning": { "effort": effort } }),
						"{id} {reasoning}"
					);
				});
			}
		}
	}

	#[test]
	fn sends_reasoning_effort_when_only_model_name_identifies_a_gpt_model() {
		run(|_host| async move {
			let model = model_with(
				"global.openai.gpt-6-sol",
				"arn:aws:bedrock:us-east-1:123456789012:application-inference-profile/my-profile",
				"GPT-6 Sol",
			);

			let payload = capture_payload(&model, with_reasoning("medium")).await;

			assert_eq!(
				payload["additionalModelRequestFields"],
				json!({ "reasoning": { "effort": "medium" } })
			);
		});
	}

	#[test]
	fn sends_flat_reasoning_effort_for_gpt_oss_clamped_to_high() {
		run(|_host| async move {
			let model = builtin_model("openai.gpt-oss-120b-1:0");

			assert_eq!(
				capture_payload(&model, with_reasoning("minimal")).await["additionalModelRequestFields"],
				json!({ "reasoning_effort": "low" })
			);
			assert_eq!(
				capture_payload(&model, with_reasoning("medium")).await["additionalModelRequestFields"],
				json!({ "reasoning_effort": "medium" })
			);
			assert_eq!(
				capture_payload(&model, with_reasoning("xhigh")).await["additionalModelRequestFields"],
				json!({ "reasoning_effort": "high" })
			);
		});
	}

	#[test]
	fn sends_no_reasoning_fields_when_reasoning_is_off() {
		run(|_host| async move {
			let captured = capture(
				&builtin_model("global.openai.gpt-6-sol"),
				&make_context(),
				BedrockOptions::default(),
			)
			.await;

			assert_eq!(captured.get("additionalModelRequestFields"), None);
		});
	}
}

mod application_inference_profile_support {
	use super::*;

	#[test]
	fn uses_adaptive_thinking_when_model_name_contains_the_model_name_but_arn_does_not() {
		run(|_host| async move {
			let model = model_with(
				"global.anthropic.claude-opus-4-6-v1",
				"arn:aws:bedrock:us-east-1:123456789012:application-inference-profile/my-profile",
				"Claude Opus 4.6",
			);

			let payload = capture_payload(&model, None).await;

			assert_eq!(
				field(&payload, "thinking"),
				json!({ "type": "adaptive", "display": "summarized" })
			);
			assert_eq!(field(&payload, "output_config"), json!({ "effort": "high" }));
		});
	}

	#[test]
	fn injects_cache_points_when_model_name_identifies_a_supported_claude_model() {
		run(|_host| async move {
			let model = model_with(
				"global.anthropic.claude-opus-4-6-v1",
				"arn:aws:bedrock:us-east-1:123456789012:application-inference-profile/my-profile",
				"Claude Sonnet 4.6",
			);
			let context = bedrock_support::context(json!({
				"systemPrompt": "You are helpful.",
				"messages": [{ "role": "user", "content": "Hello", "timestamp": 1 }],
			}));

			let captured_payload = capture(&model, &context, BedrockOptions::default()).await;

			// System prompt should have a cache point
			let system = captured_payload["system"].as_array().expect("a system prompt");
			assert_eq!(system.len(), 2);
			assert!(system[1].get("cachePoint").is_some());

			// Last user message should have a cache point
			let last_msg = captured_payload["messages"].as_array().unwrap().last().unwrap();
			let last_content = last_msg["content"].as_array().unwrap().last().unwrap();
			assert!(last_content.get("cachePoint").is_some());
		});
	}

	#[test]
	fn falls_back_to_fixed_budget_thinking_for_non_adaptive_claude_via_model_name() {
		run(|_host| async move {
			let model = model_with(
				"us.anthropic.claude-sonnet-4-5-20250929-v1:0",
				"arn:aws:bedrock:us-east-1:123456789012:application-inference-profile/my-profile",
				"Claude Sonnet 4.5",
			);

			let payload = capture_payload(&model, None).await;

			let thinking = field(&payload, "thinking");
			assert_eq!(thinking["type"], json!("enabled"));
			assert!(thinking["budget_tokens"].is_number(), "{thinking}");
			assert_eq!(
				field(&payload, "anthropic_beta"),
				json!(["interleaved-thinking-2025-05-14"])
			);
		});
	}
}
