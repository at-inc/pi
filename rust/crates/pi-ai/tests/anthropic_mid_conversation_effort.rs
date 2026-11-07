//! Port of `packages/ai/test/anthropic-mid-conversation-effort.test.ts`.
//!
//! Differences from the TS: `it.each` cases run in a loop. The fetch stub of "sends the effort and binding beta
//! headers" is the `fetch` option, a `FetchFunction` that reads the request's `anthropic-beta` header.
//! `compat?.x` reads the model's raw `compat` object, so `toBeUndefined()` means the key is absent.

mod anthropic_support;

use std::cell::RefCell;
use std::rc::Rc;

use anthropic_support::{context, event, model, sse_response};
use pi_ai::api::anthropic_messages::{AnthropicOptions, stream};
use pi_ai::types::{AssistantMessage, CacheRetention, Model, StopReason, StreamOptions};
use pi_host::testing::run;
use pi_host::{FetchFunction, PiError};
use serde_json::{Value, json};

/// `managedModel(provider)`.
fn managed_model() -> Model {
	serde_json::from_value(json!({
		"id": "claude-fable-5-1",
		"name": "Claude Fable 5.1",
		"api": "anthropic-messages",
		"provider": "anthropic",
		"baseUrl": "http://127.0.0.1:9",
		"reasoning": true,
		"thinkingLevelMap": { "off": null, "minimal": "low", "low": "low", "medium": "medium", "high": "high", "max": "max" },
		"input": ["text"],
		"cost": { "input": 0, "output": 0, "cacheRead": 0, "cacheWrite": 0 },
		"contextWindow": 200000,
		"maxTokens": 32000,
		"compat": { "forceAdaptiveThinking": true, "supportsMidConvoEffort": true },
	}))
	.unwrap()
}

/// `assistant(model, level)` as its JSON literal.
fn assistant(model: &Model, level: Option<&str>) -> Value {
	let mut assistant = json!({
		"role": "assistant",
		"content": [
			{ "type": "thinking", "thinking": "reasoning", "thinkingSignature": "signature" },
			{ "type": "text", "text": "answer" },
		],
		"api": "anthropic-messages",
		"provider": model.provider,
		"model": model.id,
		"usage": {
			"input": 0,
			"output": 0,
			"cacheRead": 0,
			"cacheWrite": 0,
			"totalTokens": 0,
			"cost": { "input": 0, "output": 0, "cacheRead": 0, "cacheWrite": 0, "total": 0 },
		},
		"stopReason": "stop",
		"timestamp": 1,
	});
	if let Some(level) = level {
		assistant["providerThinkingLevel"] = json!(level);
	}
	assistant
}

/// `Pick<AnthropicOptions, "effort" | "thinkingEnabled">`.
#[derive(Default)]
struct CaptureOptions {
	effort: Option<&'static str>,
	thinking_enabled: Option<bool>,
}

/// `capture(model, context, options)`.
async fn capture(model: &Model, messages: Value, options: CaptureOptions) -> (Value, AssistantMessage) {
	let payload: Rc<RefCell<Option<Value>>> = Rc::default();
	let recorder = payload.clone();
	let mut stream_options = AnthropicOptions {
		base: StreamOptions {
			cache_retention: Some(CacheRetention::None),
			..StreamOptions::default()
		},
		thinking_enabled: Some(options.thinking_enabled.unwrap_or(true)),
		effort: options.effort.map(str::to_string),
		..AnthropicOptions::default()
	};
	stream_options.api_key = Some("test-key".into());
	stream_options.on_payload = Some(Rc::new(move |value, _| {
		*recorder.borrow_mut() = Some(value);
		Box::pin(async { Err(PiError::new("payload captured")) })
	}));
	let message = stream(model, &context(json!({ "messages": messages })), Some(stream_options))
		.result()
		.await
		.unwrap();
	let payload = payload.borrow_mut().take().expect("Expected payload capture");
	(payload, message)
}

fn user(text: &str, timestamp: u64) -> Value {
	json!({ "role": "user", "content": text, "timestamp": timestamp })
}

/// `effortMessages(payload)`.
fn effort_messages(payload: &Value) -> Vec<Value> {
	payload["messages"]
		.as_array()
		.unwrap()
		.iter()
		.filter(|message| message["role"] == "system")
		.cloned()
		.collect()
}

/// `model.compat?.[key]`.
fn compat_value(model: &Model, key: &str) -> Option<Value> {
	model.compat.as_ref().and_then(|compat| compat.get(key)).cloned()
}

/// `model.thinkingLevelMap?.off === null`.
fn off_is_null(model: &Model) -> bool {
	matches!(model.thinking_level_map.as_ref().map(|map| &map.off), Some(Some(None)))
}

mod anthropic_mid_conversation_effort {
	use super::*;

	#[test]
	fn reconstructs_an_exact_historical_marker_prefix_and_appends_the_current_marker() {
		run(|_| async {
			let model = managed_model();
			let first = capture(
				&model,
				json!([user("one", 1)]),
				CaptureOptions {
					effort: Some("low"),
					..CaptureOptions::default()
				},
			)
			.await;
			let second = capture(
				&model,
				json!([user("one", 1), assistant(&model, Some("low")), user("two", 2)]),
				CaptureOptions {
					effort: Some("high"),
					..CaptureOptions::default()
				},
			)
			.await;

			let first_messages = first.0["messages"].as_array().unwrap();
			let second_messages = second.0["messages"].as_array().unwrap();
			assert_eq!(
				first.0["messages"],
				json!([
					{ "role": "user", "content": "one" },
					{ "role": "system", "content": [], "output_config": { "effort": "low" } },
				])
			);
			assert_eq!(second_messages[..first_messages.len()], first_messages[..]);
			assert_eq!(
				second_messages.last(),
				Some(&json!({ "role": "system", "content": [], "output_config": { "effort": "high" } }))
			);
			assert_eq!(first.0.get("output_config"), Some(&json!({ "effort": "low" })));
			assert_eq!(second.0.get("output_config"), Some(&json!({ "effort": "low" })));
			assert_eq!(
				second.0.get("thinking"),
				Some(&json!({
					"type": "adaptive",
					"display": "summarized",
					"block_binding": { "prefix_mismatch_behavior": "drop_block" },
				}))
			);
			assert_eq!(first.1.provider_thinking_level.as_deref(), Some("low"));
		});
	}

	#[test]
	fn preserves_native_effort() {
		for effort in ["low", "medium", "high", "xhigh", "max"] {
			run(|_| async move {
				let model = managed_model();
				let (payload, message) = capture(
					&model,
					json!([user("one", 1)]),
					CaptureOptions {
						effort: Some(effort),
						..CaptureOptions::default()
					},
				)
				.await;
				assert_eq!(
					effort_messages(&payload),
					vec![json!({ "role": "system", "content": [], "output_config": { "effort": effort } })]
				);
				assert_eq!(message.provider_thinking_level.as_deref(), Some(effort));
			});
		}
	}

	#[test]
	fn defaults_omitted_effort_to_high_and_still_enables_drop_block() {
		run(|_| async {
			let (payload, message) =
				capture(&managed_model(), json!([user("one", 1)]), CaptureOptions::default()).await;
			assert_eq!(
				payload["messages"].as_array().unwrap().last(),
				Some(&json!({ "role": "system", "content": [], "output_config": { "effort": "high" } }))
			);
			assert_eq!(
				payload["thinking"]["block_binding"]["prefix_mismatch_behavior"],
				json!("drop_block")
			);
			assert_eq!(message.provider_thinking_level.as_deref(), Some("high"));
		});
	}

	#[test]
	fn does_not_invent_markers_for_legacy_or_other_provider_assistants() {
		run(|_| async {
			let model = managed_model();
			let legacy = assistant(&model, None);
			let mut other_provider = assistant(&model, Some("low"));
			other_provider["provider"] = json!("other-provider");
			let (payload, _) = capture(
				&model,
				json!([user("one", 1), legacy, user("two", 2), other_provider, user("three", 3)]),
				CaptureOptions {
					effort: Some("medium"),
					..CaptureOptions::default()
				},
			)
			.await;
			assert_eq!(
				effort_messages(&payload),
				vec![json!({ "role": "system", "content": [], "output_config": { "effort": "medium" } })]
			);
		});
	}

	#[test]
	fn sends_plain_disabled_thinking_without_effort_when_off_is_supported() {
		run(|_| async {
			let mut model = managed_model();
			model.thinking_level_map = Some(
				serde_json::from_value(
					json!({ "minimal": null, "low": "low", "medium": "medium", "high": "high", "max": "max" }),
				)
				.unwrap(),
			);
			// Anthropic rejects block_binding with disabled thinking, and rejects effort markers
			// that change level (here low, then medium) while thinking is disabled.
			let (payload, message) = capture(
				&model,
				json!([
					user("one", 1),
					assistant(&model, Some("low")),
					user("two", 2),
					assistant(&model, Some("medium")),
					user("three", 3),
				]),
				CaptureOptions {
					thinking_enabled: Some(false),
					..CaptureOptions::default()
				},
			)
			.await;
			assert_eq!(payload.get("thinking"), Some(&json!({ "type": "disabled" })));
			assert_eq!(payload.get("output_config"), None);
			assert_eq!(effort_messages(&payload), Vec::<Value>::new());
			assert_eq!(message.provider_thinking_level, None);
		});
	}

	#[test]
	fn keeps_adaptive_thinking_when_off_is_unsupported() {
		run(|_| async {
			let (payload, message) = capture(
				&managed_model(),
				json!([user("one", 1)]),
				CaptureOptions {
					thinking_enabled: Some(false),
					..CaptureOptions::default()
				},
			)
			.await;
			assert_eq!(payload["thinking"]["type"], json!("adaptive"));
			assert_eq!(
				effort_messages(&payload),
				vec![json!({ "role": "system", "content": [], "output_config": { "effort": "high" } })]
			);
			assert_eq!(message.provider_thinking_level.as_deref(), Some("high"));
		});
	}

	#[test]
	fn leaves_unsupported_models_on_top_level_effort() {
		run(|_| async {
			let mut model = managed_model();
			model.compat = json!({ "forceAdaptiveThinking": true }).as_object().cloned();
			let (payload, message) = capture(
				&model,
				json!([user("one", 1)]),
				CaptureOptions {
					effort: Some("low"),
					..CaptureOptions::default()
				},
			)
			.await;
			assert_eq!(payload["messages"], json!([{ "role": "user", "content": "one" }]));
			assert_eq!(payload.get("output_config"), Some(&json!({ "effort": "low" })));
			assert_eq!(
				payload.get("thinking"),
				Some(&json!({ "type": "adaptive", "display": "summarized" }))
			);
			assert_eq!(message.provider_thinking_level, None);
		});
	}

	#[test]
	fn sends_the_effort_and_binding_beta_headers() {
		run(|_| async {
			let beta_header: Rc<RefCell<Option<String>>> = Rc::default();
			let recorder = beta_header.clone();
			let fetch_impl: FetchFunction = Rc::new(move |request| {
				*recorder.borrow_mut() = request
					.headers
					.iter()
					.find(|(name, _)| name.eq_ignore_ascii_case("anthropic-beta"))
					.map(|(_, value)| value.clone());
				Box::pin(async {
					Ok(sse_response(&[
						event(
							"message_start",
							json!({
								"type": "message_start",
								"message": {
									"id": "msg_test",
									"model": "claude-fable-5-1",
									"usage": { "input_tokens": 1, "output_tokens": 0 },
								},
							}),
						),
						event(
							"message_delta",
							json!({
								"type": "message_delta",
								"delta": { "stop_reason": "end_turn" },
								"usage": { "input_tokens": 1, "output_tokens": 1 },
							}),
						),
						event("message_stop", json!({ "type": "message_stop" })),
					]))
				})
			});
			let mut options = AnthropicOptions {
				base: StreamOptions {
					cache_retention: Some(CacheRetention::None),
					..StreamOptions::default()
				},
				..AnthropicOptions::default()
			};
			options.api_key = Some("test-key".into());
			options.fetch = Some(fetch_impl);
			let result = stream(
				&managed_model(),
				&context(json!({ "messages": [user("one", 1)] })),
				Some(options),
			)
			.result()
			.await
			.unwrap();

			assert_eq!(result.stop_reason, StopReason::Stop);
			let beta_header = beta_header.borrow().clone().unwrap_or_default();
			assert!(beta_header.contains("mid-conversation-output-config-2026-07-01"));
			assert!(beta_header.contains("thinking-binding-controls-2026-08-01"));
		});
	}

	#[test]
	fn generates_exact_model_and_transport_gates() {
		let direct = model("anthropic", "claude-fable-5-1");
		let open_router = model("openrouter", "anthropic/claude-fable-5.1");
		let unsupported = model("anthropic", "claude-opus-4-8");
		assert_eq!(compat_value(&direct, "supportsMidConvoEffort"), Some(json!(true)));
		assert!(off_is_null(&direct));
		assert_eq!(open_router.api, "anthropic-messages");
		assert_eq!(open_router.base_url, "https://openrouter.ai/api");
		assert_eq!(compat_value(&open_router, "supportsMidConvoEffort"), Some(json!(true)));
		assert_eq!(compat_value(&unsupported, "supportsMidConvoEffort"), None);
		// OpenRouter rejects configuration_update on Opus 5 but accepts it on Fable 5.1.
		assert_eq!(
			compat_value(
				&model("openrouter", "anthropic/claude-opus-5"),
				"supportsMidConvoEffort"
			),
			None
		);
		assert_eq!(
			compat_value(&model("anthropic", "claude-opus-5"), "allowedFallbackModels"),
			None
		);
	}

	#[test]
	fn supports_off_only_on_managed_models_that_accept_disabled_thinking() {
		for id in ["claude-opus-5", "claude-haiku-5-5"] {
			let model = model("anthropic", id);
			assert_eq!(
				compat_value(&model, "supportsMidConvoEffort"),
				Some(json!(true)),
				"{id}"
			);
			assert!(!off_is_null(&model), "{id}");
		}
		for id in ["claude-opus-5-5", "claude-sonnet-5-5", "claude-fable-5-1"] {
			assert!(off_is_null(&model("anthropic", id)), "{id}");
		}
	}
}
