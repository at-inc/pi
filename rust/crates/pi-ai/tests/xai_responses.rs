//! Port of `packages/ai/test/xai-responses.test.ts`.
//!
//! Not ported: "uses pi's User-Agent by default for Completions requests" and "lets explicit headers override the
//! default Completions User-Agent", which stream through `api::openai_completions` (w3-openai-completions, still
//! `PORT-STATUS: pending` on this branch). The `globalThis.fetch` spy is the test host's fetch.

mod openai_responses_support;

use openai_responses_support::{assert_match_object, context, response, responses_options, sse_body, with};
use pi_ai::api::openai_responses::stream as stream_open_ai_responses;
use pi_ai::models::get_supported_thinking_levels;
use pi_ai::providers::model_schema::ModelThinkingLevel;
use pi_ai::providers::xai::xai_provider;
use pi_ai::providers::xai_models::XAI_MODELS;
use pi_ai::types::{Model, ProviderStreamOptions, StopReason};
use pi_ai::utils::identity::format_user_agent;
use pi_host::testing::{RecordedRequest, TestHost, run};
use serde_json::{Map, Value, json};

fn completed_body() -> String {
	let event = json!({
		"type": "response.completed",
		"sequence_number": 0,
		"response": {
			"id": "resp_xai_test",
			"status": "completed",
			"output": [],
			"usage": {
				"input_tokens": 1,
				"output_tokens": 1,
				"total_tokens": 2,
				"input_tokens_details": { "cached_tokens": 0 },
			},
		},
	});
	format!(
		"{}data: [DONE]\n\n",
		sse_body(&[event]).replacen("event: response.completed\n", "", 1)
	)
}

fn respond_completed(host: &TestHost) {
	host.on_fetch(|_| {
		Box::pin(async {
			Ok(response(
				200,
				&[("content-type", "text/event-stream")],
				completed_body(),
			))
		})
	});
}

fn xai_model(id: &str) -> Model {
	XAI_MODELS.get(id).cloned().unwrap_or_else(|| panic!("{id} is defined"))
}

/// `xaiProvider().stream(model, normalizeContext(context), options)`, returning the recorded request.
async fn capture_request(host: &TestHost, model: &Model, context_json: Value, extra: Value) -> RecordedRequest {
	respond_completed(host);
	let mut options = ProviderStreamOptions::default();
	let Value::Object(mut extra) = extra else {
		panic!("extra options are an object");
	};
	if let Some(api_key) = extra.remove("apiKey") {
		options.api_key = api_key.as_str().map(str::to_string);
	}
	if let Some(session_id) = extra.remove("sessionId") {
		options.session_id = session_id.as_str().map(str::to_string);
	}
	if let Some(cache_retention) = extra.remove("cacheRetention") {
		options.cache_retention = Some(serde_json::from_value(cache_retention).expect("cache retention"));
	}
	if let Some(headers) = extra.remove("headers") {
		options.headers = Some(serde_json::from_value(headers).expect("headers"));
	}
	options.extra = Map::from_iter(extra);
	let result = (xai_provider().stream)(model, &context(context_json), Some(options))
		.expect("stream starts")
		.result()
		.await
		.expect("stream has a result");
	assert_eq!(result.stop_reason, StopReason::Stop, "{:?}", result.error_message);
	host.requests().pop().expect("fetch was called")
}

mod x_ai_responses_provider {
	use super::*;

	#[test]
	fn excludes_retired_and_redundant_models_from_the_built_in_catalog() {
		for model_id in [
			"grok-3",
			"grok-3-fast",
			"grok-4.20-0309-non-reasoning",
			"grok-4.20-0309-reasoning",
			"grok-build-0.1",
			"grok-code-fast-1",
		] {
			assert!(!XAI_MODELS.contains_key(model_id), "{model_id}");
		}
	}

	#[test]
	fn routes_every_built_in_x_ai_model_through_responses() {
		use ModelThinkingLevel::{High, Low, Medium, Off, Xhigh};
		for model in XAI_MODELS.values() {
			assert_eq!(model.api, "openai-responses", "{}", model.id);
		}
		assert_eq!(
			get_supported_thinking_levels(&xai_model("grok-4.5")),
			[Low, Medium, High, Xhigh]
		);
		assert_eq!(
			get_supported_thinking_levels(&xai_model("grok-4.6")),
			[Low, Medium, High, Xhigh]
		);
		assert_eq!(
			get_supported_thinking_levels(&xai_model("grok-4.7")),
			[Low, Medium, High, Xhigh]
		);
		assert_eq!(
			get_supported_thinking_levels(&xai_model("grok-4.3")),
			[Off, Low, Medium, High, Xhigh]
		);
	}

	#[test]
	fn includes_grok_4_7_capabilities_and_long_context_pricing() {
		assert_match_object(
			&serde_json::to_value(xai_model("grok-4.7")).unwrap(),
			&json!({
				"api": "openai-responses",
				"reasoning": true,
				"input": ["text", "image"],
				"contextWindow": 500000,
				"maxTokens": 500000,
				"cost": {
					"input": 2,
					"output": 6,
					"cacheRead": 0.5,
					"cacheWrite": 0,
					"tiers": [{ "inputTokensAbove": 200000, "input": 4, "output": 12, "cacheRead": 1, "cacheWrite": 0 }],
				},
			}),
		);
	}

	#[test]
	fn uses_responses_with_bearer_auth_and_x_ai_compatible_request_fields() {
		run(|host| async move {
			let captured = capture_request(
				&host,
				&xai_model("grok-4.5"),
				json!({
					"systemPrompt": "You are a careful coding assistant.",
					"messages": [{ "role": "user", "content": "hello", "timestamp": 1 }],
				}),
				json!({
					"apiKey": "xai-test-token",
					"sessionId": "pi-session-123",
					"cacheRetention": "long",
					"reasoningEffort": "medium",
				}),
			)
			.await;

			assert_eq!(captured.url, "https://api.x.ai/v1/responses");
			assert_eq!(captured.header("authorization"), Some("Bearer xai-test-token"));
			assert_eq!(captured.header("user-agent"), Some(format_user_agent(None).as_str()));
			assert_eq!(captured.header("session_id"), Some("pi-session-123"));
			let body = captured.json();
			assert_match_object(
				&body,
				&json!({
					"model": "grok-4.5",
					"store": false,
					"stream": true,
					"prompt_cache_key": "pi-session-123",
					"reasoning": { "effort": "medium" },
					"include": ["reasoning.encrypted_content"],
				}),
			);
			assert!(body.get("prompt_cache_retention").is_none());
			assert!(
				body["input"].as_array().unwrap().iter().any(|item| {
					item["role"] == "developer" && item["content"] == "You are a careful coding assistant."
				}),
				"{body}"
			);
		});
	}

	#[test]
	fn requests_encrypted_reasoning_without_an_effort_override() {
		run(|host| async move {
			let captured = capture_request(
				&host,
				&xai_model("grok-4.5"),
				json!({ "messages": [{ "role": "user", "content": "hello", "timestamp": 1 }] }),
				json!({ "apiKey": "xai-test-token" }),
			)
			.await;

			let body = captured.json();
			assert_match_object(
				&body,
				&json!({ "model": "grok-4.5", "store": false, "include": ["reasoning.encrypted_content"] }),
			);
			assert!(body.get("reasoning").is_none());
		});
	}

	#[test]
	fn uses_responses_for_grok_4_7_with_xhigh_effort_and_encrypted_reasoning() {
		run(|host| async move {
			let captured = capture_request(
				&host,
				&xai_model("grok-4.7"),
				json!({
					"systemPrompt": "You are a careful coding assistant.",
					"messages": [{ "role": "user", "content": "hello", "timestamp": 1 }],
				}),
				json!({ "apiKey": "xai-test-token", "reasoningEffort": "xhigh" }),
			)
			.await;

			assert_eq!(captured.url, "https://api.x.ai/v1/responses");
			assert_match_object(
				&captured.json(),
				&json!({
					"model": "grok-4.7",
					"store": false,
					"stream": true,
					"reasoning": { "effort": "xhigh" },
					"include": ["reasoning.encrypted_content"],
				}),
			);
		});
	}

	#[test]
	fn uses_responses_for_grok_4_3() {
		run(|host| async move {
			let captured = capture_request(
				&host,
				&xai_model("grok-4.3"),
				json!({ "messages": [{ "role": "user", "content": "hello", "timestamp": 1 }] }),
				json!({ "apiKey": "xai-test-token", "reasoningEffort": "low" }),
			)
			.await;

			assert_eq!(captured.url, "https://api.x.ai/v1/responses");
			assert_match_object(
				&captured.json(),
				&json!({
					"model": "grok-4.3",
					"store": false,
					"include": ["reasoning.encrypted_content"],
					"reasoning": { "effort": "low" },
				}),
			);
		});
	}

	#[test]
	fn uses_pi_s_user_agent_by_default_for_responses_requests() {
		run(|host| async move {
			respond_completed(&host);
			let openai_model = with(
				&xai_model("grok-4.5"),
				json!({ "provider": "openai", "baseUrl": "https://api.openai.com/v1" }),
			);
			let options = responses_options(|options| {
				options.api_key = Some("test-token".into());
			});
			let result = stream_open_ai_responses(
				&openai_model,
				&context(json!({ "messages": [{ "role": "user", "content": "hello", "timestamp": 1 }] })),
				Some(options),
			)
			.result()
			.await
			.expect("stream has a result");

			assert_eq!(result.stop_reason, StopReason::Stop, "{:?}", result.error_message);
			let requests = host.requests();
			assert_eq!(requests[0].header("user-agent"), Some(format_user_agent(None).as_str()));
		});
	}

	#[test]
	fn lets_explicit_headers_override_the_default_responses_user_agent() {
		run(|host| async move {
			let captured = capture_request(
				&host,
				&xai_model("grok-4.5"),
				json!({ "messages": [{ "role": "user", "content": "hello", "timestamp": 1 }] }),
				json!({ "apiKey": "xai-test-token", "headers": { "User-Agent": "custom-agent" } }),
			)
			.await;

			assert_eq!(captured.header("user-agent"), Some("custom-agent"));
		});
	}
}
