//! Port of `packages/ai/test/sampling-options.test.ts`.
//!
//! `DirectSamplingOptions`' `reasoningEffort` and `reasoningSummary` are API-specific keys of the direct `stream()`
//! options, so they go into `ProviderStreamOptions.extra`. `it.each` cases run in a loop.

use std::cell::RefCell;
use std::rc::Rc;

use pi_ai::compat::{stream, stream_simple};
use pi_ai::types::{Context, Model, ProviderStreamOptions, SimpleStreamOptions, StreamOptions, ThinkingLevel};
use pi_host::PiError;
use pi_host::testing::run;
use serde_json::{Map, Value, json};

fn make_context() -> Context {
	serde_json::from_value(
		json!({ "messages": [{ "role": "user", "content": "Hello", "timestamp": pi_host::now_ms() }] }),
	)
	.unwrap()
}

/// `makeModel(api, samplingParams, overrides)`.
fn make_model(api: &str, sampling_params: Option<Value>, overrides: Value) -> Model {
	let mut model = json!({
		"id": "custom-model",
		"name": "Custom Model",
		"api": api,
		"provider": "custom-provider",
		"baseUrl": "http://127.0.0.1:9/v1",
		"reasoning": false,
		"input": ["text"],
		"cost": { "input": 0, "output": 0, "cacheRead": 0, "cacheWrite": 0 },
		"contextWindow": 128000,
		"maxTokens": 16384,
	});
	if let Some(sampling_params) = sampling_params {
		model["samplingParams"] = sampling_params;
	}
	for (key, value) in overrides.as_object().expect("overrides are an object") {
		model[key] = value.clone();
	}
	serde_json::from_value(model).unwrap()
}

fn sampling(value: Value) -> Option<Map<String, Value>> {
	Some(value.as_object().expect("sampling params are an object").clone())
}

/// `capturingOptions(onCapture)` applied to `options`; the returned cell holds the captured payload.
fn capturing(options: &mut StreamOptions) -> Rc<RefCell<Option<Value>>> {
	let captured: Rc<RefCell<Option<Value>>> = Rc::default();
	let sink = captured.clone();
	options.api_key = Some("fake-key".into());
	options.on_payload = Some(Rc::new(move |payload, _| {
		*sink.borrow_mut() = Some(payload);
		Box::pin(async { Err(PiError::named("PayloadCaptured", "payload captured")) })
	}));
	captured
}

fn take_payload(captured: Rc<RefCell<Option<Value>>>) -> Value {
	captured
		.borrow_mut()
		.take()
		.expect("Expected payload to be captured before request failure")
}

/// `capturePayload(model, options)`: `extra` carries the API-specific keys of `DirectSamplingOptions`.
async fn capture_payload(model: &Model, mut base: StreamOptions, extra: Value) -> Value {
	let captured = capturing(&mut base);
	let options = ProviderStreamOptions {
		base,
		extra: extra.as_object().expect("extra options are an object").clone(),
	};
	stream(model, &make_context(), Some(options)).unwrap().result().await;
	take_payload(captured)
}

/// `captureSimplePayload(model, options)`.
async fn capture_simple_payload(model: &Model, mut options: SimpleStreamOptions) -> Value {
	let captured = capturing(&mut options.base);
	stream_simple(model, &make_context(), Some(options))
		.unwrap()
		.result()
		.await;
	take_payload(captured)
}

mod sampling_params {
	use super::*;

	#[test]
	fn merges_request_sampling_params_into_the_request_body() {
		run(|_| async {
			let options = StreamOptions {
				sampling_params: sampling(json!({ "top_p": 0.95, "top_k": 0, "min_p": 0 })),
				..StreamOptions::default()
			};
			let payload = capture_payload(&make_model("openai-completions", None, json!({})), options, json!({})).await;

			assert_eq!(payload["top_p"], json!(0.95));
			assert_eq!(payload["top_k"].as_f64(), Some(0.0));
			assert_eq!(payload["min_p"].as_f64(), Some(0.0));
		});
	}

	#[test]
	fn omits_sampling_params_when_neither_options_nor_model_set_them() {
		run(|_| async {
			let payload = capture_payload(
				&make_model("openai-completions", None, json!({})),
				StreamOptions::default(),
				json!({}),
			)
			.await;

			assert!(payload.get("temperature").is_none());
			assert!(payload.get("top_p").is_none());
		});
	}

	// Model defaults must apply to direct stream()/complete() calls, not only streamSimple() (#9506)
	#[test]
	fn applies_model_level_sampling_params_with_request_keys_taking_precedence() {
		for api in ["openai-completions", "openai-responses", "azure-openai-responses"] {
			run(|_| async move {
				let options = StreamOptions {
					sampling_params: sampling(json!({ "top_p": 0.5 })),
					..StreamOptions::default()
				};
				let model = make_model(api, Some(json!({ "top_p": 0.95, "min_p": 0.05 })), json!({}));
				let payload = capture_payload(&model, options, json!({})).await;

				assert_eq!(payload["top_p"], json!(0.5), "{api}");
				assert_eq!(payload["min_p"], json!(0.05), "{api}");
			});
		}
	}

	#[test]
	fn passes_request_sampling_params_through_stream_simple() {
		run(|_| async {
			let options = SimpleStreamOptions {
				base: StreamOptions {
					sampling_params: sampling(json!({ "top_p": 0.5 })),
					..StreamOptions::default()
				},
				..SimpleStreamOptions::default()
			};
			let payload = capture_simple_payload(&make_model("openai-completions", None, json!({})), options).await;

			assert_eq!(payload["top_p"], json!(0.5));
		});
	}

	#[test]
	fn applies_sampling_params_for_the_effective_thinking_level_over_model_defaults() {
		run(|_| async {
			let model = make_model(
				"openai-completions",
				Some(json!({ "temperature": 1, "top_p": 0.95 })),
				json!({
					"reasoning": true,
					"thinkingLevelMap": { "low": null, "medium": null },
					"samplingParamsByThinkingLevel": { "high": { "temperature": 0.8, "top_k": 64 } },
				}),
			);
			let options = SimpleStreamOptions {
				reasoning: Some(ThinkingLevel::Low),
				..SimpleStreamOptions::default()
			};
			let payload = capture_simple_payload(&model, options).await;

			assert_eq!(payload["temperature"], json!(0.8));
			assert_eq!(payload["top_p"], json!(0.95));
			assert_eq!(payload["top_k"].as_f64(), Some(64.0));
		});
	}

	#[test]
	fn applies_off_sampling_params_when_reasoning_is_disabled() {
		run(|_| async {
			let model = make_model(
				"openai-completions",
				None,
				json!({ "samplingParamsByThinkingLevel": { "off": { "temperature": 0.7 } } }),
			);
			let payload = capture_simple_payload(&model, SimpleStreamOptions::default()).await;

			assert_eq!(payload["temperature"], json!(0.7));
		});
	}

	#[test]
	fn merges_stream_option_keys_over_thinking_level_keys() {
		run(|_| async {
			let model = make_model(
				"openai-completions",
				None,
				json!({
					"reasoning": true,
					"samplingParamsByThinkingLevel": { "low": { "temperature": 0.6, "top_p": 0.95 } },
				}),
			);
			let mut options = SimpleStreamOptions {
				reasoning: Some(ThinkingLevel::Low),
				..SimpleStreamOptions::default()
			};
			options.sampling_params = sampling(json!({ "top_p": 0.5 }));
			let payload = capture_simple_payload(&model, options).await;

			assert_eq!(payload["temperature"], json!(0.6));
			assert_eq!(payload["top_p"], json!(0.5));
		});
	}

	#[test]
	fn applies_thinking_level_params_between_model_and_request_params() {
		for api in ["openai-completions", "openai-responses", "azure-openai-responses"] {
			run(|_| async move {
				let model = make_model(
					api,
					Some(json!({ "temperature": 1, "top_p": 0.95 })),
					json!({
						"reasoning": true,
						"samplingParamsByThinkingLevel": { "low": { "temperature": 0.6, "top_k": 64 } },
					}),
				);
				let options = StreamOptions {
					sampling_params: sampling(json!({ "top_p": 0.5 })),
					..StreamOptions::default()
				};
				let payload = capture_payload(&model, options, json!({ "reasoningEffort": "low" })).await;

				assert_eq!(payload["temperature"], json!(0.6), "{api}");
				assert_eq!(payload["top_p"], json!(0.5), "{api}");
				assert_eq!(payload["top_k"].as_f64(), Some(64.0), "{api}");
			});
		}
	}

	#[test]
	fn uses_medium_sampling_params_for_summary_only_requests() {
		for api in ["openai-responses", "azure-openai-responses"] {
			run(|_| async move {
				let model = make_model(
					api,
					None,
					json!({
						"reasoning": true,
						"samplingParamsByThinkingLevel": {
							"off": { "temperature": 0.7 },
							"medium": { "temperature": 0.8 },
						},
					}),
				);
				let payload =
					capture_payload(&model, StreamOptions::default(), json!({ "reasoningSummary": "auto" })).await;

				assert_eq!(payload["reasoning"]["effort"], "medium", "{api}");
				assert_eq!(payload["temperature"], json!(0.8), "{api}");
			});
		}
	}

	#[test]
	fn overrides_named_request_fields() {
		run(|_| async {
			let options = StreamOptions {
				temperature: Some(0.0),
				sampling_params: sampling(json!({ "temperature": 1 })),
				..StreamOptions::default()
			};
			let payload = capture_payload(&make_model("openai-completions", None, json!({})), options, json!({})).await;

			assert_eq!(payload["temperature"].as_f64(), Some(1.0));
		});
	}

	#[test]
	fn is_ignored_by_non_openai_compatible_apis() {
		run(|_| async {
			let options = StreamOptions {
				sampling_params: sampling(json!({ "top_p": 0.9, "top_k": 40 })),
				..StreamOptions::default()
			};
			let payload = capture_payload(&make_model("anthropic-messages", None, json!({})), options, json!({})).await;
			assert!(payload.get("top_p").is_none());
			assert!(payload.get("top_k").is_none());
		});
	}
}
