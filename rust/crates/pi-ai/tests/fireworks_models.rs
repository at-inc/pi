//! Port of `packages/ai/test/fireworks-models.test.ts`.
//!
//! `getModel` (compat.ts) is `get_builtin_model`. `process.env` is the `TestHost` environment, so the `afterEach`
//! restore is not needed. `it.each` cases run in a loop.
//!
//! The `Anthropic-compatible session affinity and tool compat` describe captures requests through the test host's
//! `fetch` instead of a local `node:http` server; the model's `baseUrl` override is not needed.

use std::cell::RefCell;
use std::rc::Rc;

use indexmap::IndexMap;
use pi_ai::api::anthropic_messages::{AnthropicOptions, stream as stream_anthropic};
use pi_ai::compat::stream_simple;
use pi_ai::env_api_keys::{find_env_keys, get_env_api_key};
use pi_ai::models::get_supported_thinking_levels;
use pi_ai::providers::all::get_builtin_model;
use pi_ai::providers::model_schema::ModelThinkingLevel;
use pi_ai::types::{
	CacheRetention, Context, Model, ModelInputModality, ProviderRequestOptions, SimpleStreamOptions, StreamOptions,
};
use pi_ai::utils::transcript::normalize_context;
use pi_host::testing::RecordedRequest;
use pi_host::{HttpResponse, PiError};
use serde_json::{Value, json};

/// `expect(actual).toMatchObject(expected)` for JSON values; numbers compare as `f64`, like JS numbers.
fn assert_match_object(actual: &Value, expected: &Value) {
	match (actual, expected) {
		(Value::Object(actual), Value::Object(expected)) => {
			for (key, expected) in expected {
				assert_match_object(actual.get(key).unwrap_or_else(|| panic!("missing key {key}")), expected);
			}
		}
		(Value::Array(actual), Value::Array(expected)) => {
			assert_eq!(actual.len(), expected.len());
			for (actual, expected) in actual.iter().zip(expected) {
				assert_match_object(actual, expected);
			}
		}
		(Value::Number(actual), Value::Number(expected)) => assert_eq!(actual.as_f64(), expected.as_f64()),
		_ => assert_eq!(actual, expected),
	}
}

/// `expect(actual).toEqual(expected)` for JSON objects: the same keys, values as in `assert_match_object`.
fn assert_json_eq(actual: &Value, expected: &Value) {
	assert_eq!(
		actual
			.as_object()
			.map(|object| object.keys().collect::<std::collections::BTreeSet<_>>()),
		expected
			.as_object()
			.map(|object| object.keys().collect::<std::collections::BTreeSet<_>>())
	);
	assert_match_object(actual, expected);
}

use ModelThinkingLevel::{High, Low, Max, Medium, Off, Xhigh};

fn fireworks(model_id: &str) -> Model {
	get_builtin_model("fireworks", model_id).unwrap_or_else(|| panic!("fireworks/{model_id} is missing"))
}

fn to_json(value: impl serde::Serialize) -> Value {
	serde_json::to_value(value).unwrap()
}

fn test_context() -> Context {
	serde_json::from_value(json!({ "messages": [{ "role": "user", "content": "test", "timestamp": 0 }] })).unwrap()
}

/// `streamSimple` with `reasoning` (`off` is `undefined`) and an `onPayload` that throws; returns the payload.
async fn capture_simple_payload(model: &Model, reasoning: Option<ModelThinkingLevel>) -> Value {
	capture_payload_with(model, reasoning, |_| {}).await
}

/// `capture_simple_payload` with further options set by `configure`.
async fn capture_payload_with(
	model: &Model,
	reasoning: Option<ModelThinkingLevel>,
	configure: impl FnOnce(&mut SimpleStreamOptions),
) -> Value {
	let captured: Rc<RefCell<Option<Value>>> = Rc::default();
	let sink = captured.clone();
	let base = StreamOptions {
		base: ProviderRequestOptions {
			api_key: Some("test-fireworks-key".into()),
			on_payload: Some(Rc::new(move |payload, _| {
				*sink.borrow_mut() = Some(payload);
				Box::pin(async { Err(PiError::new("payload captured")) })
			})),
			..ProviderRequestOptions::default()
		},
		..StreamOptions::default()
	};
	let options = SimpleStreamOptions {
		base,
		reasoning: reasoning
			.filter(|level| *level != Off)
			.map(|level| serde_json::from_value(to_json(level)).unwrap()),
		..SimpleStreamOptions::default()
	};
	let mut options = options;
	configure(&mut options);
	stream_simple(model, &test_context(), Some(options))
		.unwrap()
		.result()
		.await;
	captured.borrow_mut().take().expect("payload is defined")
}

mod fireworks_models {
	use super::*;

	#[test]
	fn registers_non_glm_non_kimi_k3_models_via_anthropic_compatible_messages_api() {
		let model = fireworks("accounts/fireworks/models/deepseek-v4p1-flash");

		assert_eq!(model.api, "anthropic-messages");
		assert_eq!(model.provider, "fireworks");
		assert_eq!(model.base_url, "https://api.fireworks.ai/inference");
		assert!(model.reasoning);
		assert_eq!(model.input, [ModelInputModality::Text, ModelInputModality::Image]);
	}

	#[test]
	fn aligns_glm_5_3_fast_with_glm_5_3s_open_ai_compatible_config() {
		let base = fireworks("accounts/fireworks/models/glm-5p3");
		let fast = fireworks("accounts/fireworks/routers/glm-5p3-fast");

		assert_eq!(fast.api, base.api);
		assert_eq!(fast.base_url, base.base_url);
		assert_eq!(fast.compat, base.compat);
		assert_eq!(fast.thinking_level_map, base.thinking_level_map);
	}

	#[test]
	fn omits_unsupported_long_cache_retention() {
		for model_id in [
			"accounts/fireworks/models/glm-5p3",
			"accounts/fireworks/routers/glm-5p3-fast",
		] {
			pi_host::testing::run(|_| async move {
				let payload = capture_payload_with(&fireworks(model_id), None, |options| {
					options.cache_retention = Some(CacheRetention::Long);
					options.session_id = Some("test-fireworks-session".into());
				})
				.await;

				assert!(payload.get("prompt_cache_retention").is_none(), "{model_id}");
			});
		}
	}

	#[test]
	fn routes_kimi_k3_through_the_open_ai_compatible_api_with_native_effort_controls() {
		let base = fireworks("accounts/fireworks/models/kimi-k3");
		let fast = fireworks("accounts/fireworks/routers/kimi-k3-fast");
		let compat = json!({
			"supportsStore": false,
			"supportsDeveloperRole": false,
			"supportsStrictMode": true,
			"requiresReasoningContentOnAssistantMessages": true,
			"thinkingFormat": "openai",
			"supportsMidConvoSystemMessages": true,
			"supportsMidConvoToolAdditions": true,
			"sendSessionAffinityHeaders": true,
			"supportsLongCacheRetention": false,
		});
		let thinking_level_map = json!({
			"off": null,
			"minimal": null,
			"low": "low",
			"medium": null,
			"high": "high",
			"xhigh": null,
			"max": "max",
		});

		assert_eq!(base.api, "openai-completions");
		assert_eq!(base.base_url, "https://api.fireworks.ai/inference/v1");
		assert_json_eq(&to_json(&base.compat), &compat);
		assert_json_eq(&to_json(&base.thinking_level_map), &thinking_level_map);
		assert_eq!(fast.api, base.api);
		assert_eq!(fast.base_url, base.base_url);
		assert_json_eq(&to_json(&fast.compat), &compat);
		assert_json_eq(&to_json(&fast.thinking_level_map), &thinking_level_map);

		pi_host::testing::run(|_| async {
			let payload = capture_simple_payload(&base, Some(Max)).await;
			assert_eq!(payload["reasoning_effort"], "max");
		});
	}

	// Regression for #9323: native effort must reach Messages without budget-based fallback.
	#[test]
	fn sends_native_messages_effort_levels() {
		for (model_id, levels) in [
			(
				"accounts/fireworks/models/deepseek-v4p1-flash",
				vec![Off, Low, High, Max],
			),
			("accounts/fireworks/models/qwen3p8-max", vec![Off, Low, Medium, Xhigh]),
			(
				"accounts/fireworks/models/qwen3p8-2p4t-a95b",
				vec![Off, Low, Medium, Xhigh],
			),
		] {
			let model = fireworks(model_id);
			assert_eq!(model.api, "anthropic-messages", "{model_id}");
			assert_eq!(
				to_json(&model.compat)["forceAdaptiveThinking"],
				json!(true),
				"{model_id}"
			);
			assert_eq!(get_supported_thinking_levels(&model), levels, "{model_id}");

			pi_host::testing::run(|_| async {
				for level in &levels {
					let payload = capture_simple_payload(&model, Some(*level)).await;
					let expected_thinking = if *level == Off {
						json!({ "type": "disabled" })
					} else {
						json!({ "type": "adaptive", "display": "summarized" })
					};
					assert_eq!(payload["thinking"], expected_thinking, "{model_id} {level:?}");
					let expected_output_config = if *level == Off {
						None
					} else {
						Some(json!({ "effort": to_json(level) }))
					};
					assert_eq!(
						payload.get("output_config").cloned(),
						expected_output_config,
						"{model_id} {level:?}"
					);
				}
			});
		}
	}

	// Regression for #9323: accepted aliases are not distinct native effort levels.
	#[test]
	fn exposes_distinct_native_effort_levels() {
		for model_id in [
			"accounts/fireworks/models/glm-5p3",
			"accounts/fireworks/routers/glm-5p3-fast",
			"accounts/fireworks/models/kimi-k3",
			"accounts/fireworks/routers/kimi-k3-fast",
		] {
			assert_eq!(
				get_supported_thinking_levels(&fireworks(model_id)),
				[Low, High, Max],
				"{model_id}"
			);
		}
	}

	#[test]
	fn keeps_toggle_only_messages_models_without_a_verified_fallback_on_budget_based_thinking() {
		let model = fireworks("accounts/fireworks/models/nemotron-3-ultra-nvfp4");
		assert_eq!(to_json(&model.compat).get("forceAdaptiveThinking"), None);
		pi_host::testing::run(|_| async {
			let payload = capture_simple_payload(&model, Some(High)).await;
			assert_eq!(
				payload["thinking"],
				json!({ "type": "enabled", "budget_tokens": 16384, "display": "summarized" })
			);
			assert_eq!(payload.get("output_config"), None);
		});
	}

	#[test]
	fn resolves_fireworks_api_key_from_the_environment() {
		pi_host::testing::run(|host| async move {
			host.set_env("FIREWORKS_API_KEY", "test-fireworks-key");

			assert_eq!(
				find_env_keys("fireworks", None),
				Some(vec!["FIREWORKS_API_KEY".to_string()])
			);
			assert_eq!(
				get_env_api_key("fireworks", None).as_deref(),
				Some("test-fireworks-key")
			);
		});
	}

	#[test]
	fn sets_fireworks_specific_compat_for_session_affinity_and_unsupported_tool_fields() {
		let compat = to_json(&fireworks("accounts/fireworks/models/nemotron-3-ultra-nvfp4").compat);

		assert!(compat.is_object());
		assert_eq!(compat["sendSessionAffinityHeaders"], json!(true));
		assert_eq!(compat["supportsEagerToolInputStreaming"], json!(false));
		assert_eq!(compat["supportsCacheControlOnTools"], json!(false));
		assert_eq!(compat["supportsLongCacheRetention"], json!(false));
		assert_eq!(compat["allowEmptySignature"], json!(true));
	}
}

// --- Integration tests for Fireworks Anthropic session affinity and tool compat ---

fn lookup_tool() -> Value {
	json!({
		"name": "lookup",
		"description": "Look up a value",
		"parameters": { "type": "object", "properties": { "value": { "type": "string" } }, "required": ["value"] },
	})
}

fn fireworks_anthropic_compat() -> Value {
	json!({
		"allowEmptySignature": true,
		"sendSessionAffinityHeaders": true,
		"supportsEagerToolInputStreaming": false,
		"supportsCacheControlOnTools": false,
		"supportsLongCacheRetention": false,
	})
}

fn create_fireworks_model() -> Model {
	serde_json::from_value(json!({
		"id": "accounts/fireworks/models/kimi-k2p6",
		"name": "Kimi K2.6",
		"api": "anthropic-messages",
		"provider": "fireworks",
		"baseUrl": "http://127.0.0.1:0",
		"reasoning": true,
		"input": ["text", "image"],
		"cost": { "input": 0.95, "output": 4, "cacheRead": 0.16, "cacheWrite": 0 },
		"contextWindow": 262000,
		"maxTokens": 262000,
		"compat": fireworks_anthropic_compat(),
	}))
	.unwrap()
}

fn create_anthropic_model() -> Model {
	serde_json::from_value(json!({
		"id": "claude-opus-4-8",
		"name": "Claude Opus 4.8",
		"api": "anthropic-messages",
		"provider": "anthropic",
		"baseUrl": "http://127.0.0.1:0",
		"reasoning": true,
		"input": ["text"],
		"cost": { "input": 0, "output": 0, "cacheRead": 0, "cacheWrite": 0 },
		"contextWindow": 200000,
		"maxTokens": 32000,
	}))
	.unwrap()
}

fn create_open_router_model() -> Model {
	let mut model = create_anthropic_model();
	model.id = "anthropic/claude-opus-4.8".into();
	model.provider = "openrouter".into();
	model.base_url = "https://openrouter.ai/api".into();
	model
}

fn create_context() -> Context {
	serde_json::from_value(json!({
		"messages": [{ "role": "user", "content": "Use the tool", "timestamp": 1 }],
		"tools": [lookup_tool()],
	}))
	.unwrap()
}

/// Streams `model` against an empty SSE response and returns the one request it sent.
fn capture_anthropic_request(
	model: &Model,
	session_id: Option<&str>,
	cache_retention: Option<CacheRetention>,
) -> RecordedRequest {
	pi_host::testing::run(|host| {
		host.on_fetch(|_| {
			let headers = IndexMap::from([("content-type".to_string(), "text/event-stream".to_string())]);
			Box::pin(async move { Ok(HttpResponse::from_bytes(200, headers, "")) })
		});
		let mut base = StreamOptions {
			cache_retention: Some(cache_retention.unwrap_or(CacheRetention::Short)),
			session_id: session_id.map(str::to_string),
			..StreamOptions::default()
		};
		base.api_key = Some("test-key".into());
		let options = AnthropicOptions {
			base,
			..AnthropicOptions::default()
		};
		let events = stream_anthropic(model, &normalize_context(&create_context()), Some(options));
		async move {
			events.result().await;
			let requests = host.requests();
			requests.into_iter().next().expect("Anthropic request was not captured")
		}
	})
}

fn get_tools(request: &RecordedRequest) -> Vec<Value> {
	request.json()["tools"]
		.as_array()
		.expect("Expected tools in request body")
		.clone()
}

mod anthropic_compatible_session_affinity_and_tool_compat {
	use super::*;

	#[test]
	fn sends_x_session_affinity_header_for_fireworks_models() {
		let request = capture_anthropic_request(&create_fireworks_model(), Some("fireworks-session-1"), None);
		assert_eq!(
			request.headers.get("x-session-affinity").map(String::as_str),
			Some("fireworks-session-1")
		);
	}

	#[test]
	fn omits_x_session_affinity_header_for_native_anthropic_models() {
		let request = capture_anthropic_request(&create_anthropic_model(), Some("anthropic-session-1"), None);
		assert_eq!(request.headers.get("x-session-affinity"), None);
	}

	#[test]
	fn omits_x_session_affinity_header_when_cache_retention_is_none() {
		let request = capture_anthropic_request(
			&create_fireworks_model(),
			Some("fireworks-session-2"),
			Some(CacheRetention::None),
		);
		assert_eq!(request.headers.get("x-session-affinity"), None);
	}

	// Regression test for https://github.com/earendil-works/pi/issues/9102
	#[test]
	fn sends_only_x_session_id_for_open_router_models() {
		let request = capture_anthropic_request(&create_open_router_model(), Some("openrouter-session-1"), None);
		assert_eq!(
			request.headers.get("x-session-id").map(String::as_str),
			Some("openrouter-session-1")
		);
		assert_eq!(request.headers.get("x-session-affinity"), None);
	}

	#[test]
	fn omits_open_router_session_headers_when_cache_retention_is_none() {
		let request = capture_anthropic_request(
			&create_open_router_model(),
			Some("openrouter-session-2"),
			Some(CacheRetention::None),
		);
		assert_eq!(request.headers.get("x-session-id"), None);
		assert_eq!(request.headers.get("x-session-affinity"), None);
	}

	#[test]
	fn allows_open_router_session_headers_to_be_disabled() {
		let mut model = create_open_router_model();
		model.compat = json!({ "sendSessionAffinityHeaders": false }).as_object().cloned();
		let request = capture_anthropic_request(&model, Some("openrouter-session-3"), None);
		assert_eq!(request.headers.get("x-session-id"), None);
	}

	#[test]
	fn omits_cache_control_on_tools_for_fireworks_models() {
		let request = capture_anthropic_request(&create_fireworks_model(), None, None);
		let tools = get_tools(&request);
		assert_eq!(tools.last().unwrap().get("cache_control"), None);
	}

	#[test]
	fn omits_eager_input_streaming_on_tools_for_fireworks_models() {
		let request = capture_anthropic_request(&create_fireworks_model(), None, None);
		for tool in get_tools(&request) {
			assert_eq!(tool.get("eager_input_streaming"), None);
		}
	}

	#[test]
	fn sends_cache_control_on_tools_for_native_anthropic_models() {
		let request = capture_anthropic_request(&create_anthropic_model(), None, None);
		let tools = get_tools(&request);
		let last_tool = tools.last().unwrap();
		assert_eq!(last_tool["cache_control"]["type"], "ephemeral");
	}

	#[test]
	fn sends_eager_input_streaming_on_tools_for_native_anthropic_models() {
		let request = capture_anthropic_request(&create_anthropic_model(), None, None);
		assert_eq!(get_tools(&request)[0]["eager_input_streaming"], json!(true));
	}
}
