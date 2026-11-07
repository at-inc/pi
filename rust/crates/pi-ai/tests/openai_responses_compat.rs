//! Port of `packages/ai/test/openai-responses-compat.test.ts`.
//!
//! The `globalThis.fetch` spy is the test host's fetch: request headers are read from `host.requests()`. typebox
//! `Type.Object(...)` parameters are written as the JSON schema they produce.

mod openai_responses_support;

use openai_responses_support::{assert_match_object, capture_payload, context, response, responses_options, with};
use pi_ai::api::openai_responses::{OpenAIResponsesOptions, stream as stream_open_ai_responses};
use pi_ai::providers::all::get_builtin_model;
use pi_ai::types::{AssistantMessageEvent, CacheRetention, Model, ProviderHeaders};
use pi_host::testing::{TestHost, run};
use serde_json::{Value, json};

fn get_model(provider: &str, model_id: &str) -> Model {
	get_builtin_model(provider, model_id).unwrap_or_else(|| panic!("{provider}/{model_id} is defined"))
}

fn respond_done(host: &TestHost) {
	respond_sse(host, "data: [DONE]\n\n".to_string());
}

fn respond_sse(host: &TestHost, body: String) {
	host.on_fetch(move |_| {
		let body = body.clone();
		Box::pin(async move { Ok(response(200, &[("content-type", "text/event-stream")], body)) })
	});
}

fn sys_hi() -> pi_ai::types::TranscriptContext {
	context(json!({
		"systemPrompt": "sys",
		"messages": [{ "role": "user", "content": "hi", "timestamp": pi_host::now_ms() }],
	}))
}

fn options_with(configure: impl FnOnce(&mut OpenAIResponsesOptions)) -> OpenAIResponsesOptions {
	responses_options(|options| {
		options.api_key = Some("sk-test-key".into());
		configure(options);
	})
}

/// `for await (const event of stream) { if (event.type === "done" || event.type === "error") break; }`
async fn drain(model: &Model, context: &pi_ai::types::TranscriptContext, options: OpenAIResponsesOptions) {
	let stream = stream_open_ai_responses(model, context, Some(options));
	while let Some(event) = stream.next().await {
		if matches!(
			event,
			AssistantMessageEvent::Done { .. } | AssistantMessageEvent::Error { .. }
		) {
			break;
		}
	}
}

/// Runs `model` with `options` and returns the payload it sent.
async fn capture(model: &Model, context: &pi_ai::types::TranscriptContext, options: OpenAIResponsesOptions) -> Value {
	let (on_payload, payload) = capture_payload();
	let mut options = options;
	options.on_payload = Some(on_payload);
	drain(model, context, options).await;
	payload.borrow_mut().take().expect("payload was captured")
}

struct CapturedHeaders {
	session_id: Option<String>,
	client_request_id: Option<String>,
	x_session_id: Option<String>,
}

async fn capture_open_ai_response_headers(
	host: &TestHost,
	options: OpenAIResponsesOptions,
	model: Option<Model>,
) -> CapturedHeaders {
	respond_done(host);
	let model = model.unwrap_or_else(|| get_model("openai", "gpt-5.4"));
	drain(&model, &sys_hi(), options).await;
	let requests = host.requests();
	let request = requests.last().expect("fetch was called");
	let header = |name: &str| request.header(name).map(str::to_string);
	CapturedHeaders {
		session_id: header("session_id"),
		client_request_id: header("x-client-request-id"),
		x_session_id: header("x-session-id"),
	}
}

fn has_reasoning(payload: &Value) -> bool {
	payload.get("reasoning").is_some_and(|reasoning| !reasoning.is_null())
}

mod openai_responses_provider_defaults {
	use super::*;

	// #2567: Copilot rejected reasoning.effort "none". gpt-6-sol has off: "none", so this exercises the Copilot exception.
	#[test]
	fn omits_reasoning_when_no_reasoning_is_requested() {
		run(|host| async move {
			respond_done(&host);
			let payload = capture(
				&get_model("github-copilot", "gpt-6-sol"),
				&sys_hi(),
				options_with(|_| {}),
			)
			.await;

			assert!(!payload.is_null());
			assert!(!has_reasoning(&payload), "{payload}");
		});
	}

	#[test]
	fn forwards_required_tool_choice() {
		run(|host| async move {
			respond_done(&host);
			let context = context(json!({
				"messages": [{
					"role": "user",
					"content": "Do not call ping. Respond with text instead.",
					"timestamp": pi_host::now_ms(),
				}],
				"tools": [{
					"name": "ping",
					"description": "Ping",
					"parameters": {
						"type": "object",
						"properties": { "value": { "type": "string" } },
						"required": ["value"],
					},
				}],
			}));
			let payload = capture(
				&get_model("openai", "gpt-5.6-luna"),
				&context,
				options_with(|options| options.tool_choice = Some(json!("required"))),
			)
			.await;

			// gpt-5.6-luna declares its tools in additional_tools items; tool_choice applies to them.
			assert_eq!(payload["tool_choice"], "required");
			let input = payload["input"].as_array().expect("input is an array");
			assert_eq!(input.len(), 2);
			assert_eq!(input[0]["type"], "additional_tools");
			let tools = input[0]["tools"].as_array().expect("tools is an array");
			assert_eq!(tools.len(), 1);
			assert_eq!(tools[0]["name"], "ping");
			assert!(!input[1].is_null());
		});
	}

	#[test]
	fn sets_strict_mode_explicitly_for_cloudflare_open_ai_responses_tools() {
		run(|host| async move {
			respond_done(&host);
			let model = get_model("cloudflare-ai-gateway", "gpt-5.6-sol");
			let context = context(json!({
				"messages": [{ "role": "user", "content": "Use a tool.", "timestamp": pi_host::now_ms() }],
				"tools": [
					{
						"name": "ordinary",
						"description": "An ordinary tool",
						"parameters": {
							"type": "object",
							"properties": { "path": { "type": "string" }, "offset": { "type": "number" } },
							"required": ["path"],
						},
					},
					{
						"name": "constrained",
						"description": "A constrained tool",
						"parameters": {
							"type": "object",
							"properties": { "value": { "type": "string" } },
							"required": ["value"],
						},
						"constrainedSampling": { "type": "json_schema", "strict": "prefer" },
					},
				],
			}));
			let payload = capture(&model, &context, options_with(|_| {})).await;

			assert_eq!(
				model
					.compat
					.as_ref()
					.and_then(|compat| compat.get("supportsStrictMode")),
				Some(&json!(true))
			);
			let tools = payload["tools"].as_array().expect("tools is an array");
			assert_eq!(tools.len(), 2);
			assert_match_object(&tools[0], &json!({ "name": "ordinary", "strict": false }));
			assert_match_object(&tools[1], &json!({ "name": "constrained", "strict": true }));
		});
	}

	#[test]
	fn sends_none_reasoning_effort_for_open_ai_when_no_reasoning_is_requested() {
		for model_id in [
			"gpt-5.1",
			"gpt-5.2",
			"gpt-5.3-codex",
			"gpt-5.4",
			"gpt-5.4-mini",
			"gpt-5.4-nano",
			"gpt-5.5",
			"gpt-5.6-sol",
			"gpt-5.6-terra",
			"gpt-5.6-luna",
			"gpt-6-sol",
			"gpt-6-luna",
		] {
			run(|host| async move {
				respond_done(&host);
				let payload = capture(&get_model("openai", model_id), &sys_hi(), options_with(|_| {})).await;

				assert_eq!(payload["reasoning"]["effort"], "none", "{model_id}");
			});
		}
	}

	#[test]
	fn omits_reasoning_effort_for_open_ai_when_off_is_unsupported() {
		for model_id in [
			"gpt-5",
			"gpt-5-mini",
			"gpt-5-nano",
			"gpt-5-pro",
			"gpt-5.2-pro",
			"gpt-5.4-pro",
			"gpt-5.5-pro",
			"gpt-6-astra",
			"gpt-6.1-sol",
		] {
			run(|host| async move {
				respond_done(&host);
				let payload = capture(&get_model("openai", model_id), &sys_hi(), options_with(|_| {})).await;

				assert!(!has_reasoning(&payload), "{model_id}: {payload}");
			});
		}
	}

	#[test]
	fn sets_cache_affinity_headers_for_official_open_ai_responses_requests_with_a_session_id() {
		run(|host| async move {
			let captured = capture_open_ai_response_headers(
				&host,
				options_with(|options| options.session_id = Some("session-123".into())),
				None,
			)
			.await;

			assert_eq!(captured.session_id.as_deref(), Some("session-123"));
			assert_eq!(captured.client_request_id.as_deref(), Some("session-123"));
		});
	}

	#[test]
	fn clamps_prompt_cache_key_to_open_ai_s_64_character_limit() {
		run(|host| async move {
			respond_done(&host);
			let session_id = "x".repeat(67);
			let payload = capture(
				&get_model("openai", "gpt-5.4"),
				&sys_hi(),
				options_with(|options| options.session_id = Some(session_id)),
			)
			.await;

			assert_eq!(payload["prompt_cache_key"], json!("x".repeat(64)));
		});
	}

	#[test]
	fn sets_cache_affinity_headers_for_proxy_open_ai_responses_requests_with_a_session_id() {
		run(|host| async move {
			let proxy_model = with(
				&get_model("openai", "gpt-5.4"),
				json!({ "provider": "opencode", "baseUrl": "https://proxy.example.com/v1" }),
			);
			let captured = capture_open_ai_response_headers(
				&host,
				options_with(|options| options.session_id = Some("session-123".into())),
				Some(proxy_model),
			)
			.await;

			assert_eq!(captured.session_id.as_deref(), Some("session-123"));
			assert_eq!(captured.client_request_id.as_deref(), Some("session-123"));
		});
	}

	/// Runs a session-affinity case: the captured headers and payload.
	async fn affinity(host: &TestHost, session_id: &str, model: Model) -> (CapturedHeaders, Value) {
		let (on_payload, payload) = capture_payload();
		let captured = capture_open_ai_response_headers(
			host,
			options_with(|options| {
				options.session_id = Some(session_id.into());
				options.on_payload = Some(on_payload);
			}),
			Some(model),
		)
		.await;
		let payload = payload.borrow_mut().take().expect("payload was captured");
		(captured, payload)
	}

	#[test]
	fn uses_open_router_session_affinity_header_when_configured() {
		run(|host| async move {
			let proxy_model = with(
				&get_model("openai", "gpt-5.4"),
				json!({
					"provider": "proxy",
					"baseUrl": "https://proxy.example.com/v1",
					"compat": { "sessionAffinityFormat": "openrouter" },
				}),
			);
			let (captured, payload) = affinity(&host, "session-proxy", proxy_model).await;

			assert_eq!(captured.session_id, None);
			assert_eq!(captured.client_request_id, None);
			assert_eq!(captured.x_session_id.as_deref(), Some("session-proxy"));
			assert!(payload.get("session_id").is_none());
			assert_eq!(payload["prompt_cache_key"], "session-proxy");
		});
	}

	#[test]
	fn auto_detects_open_router_session_affinity_header_for_open_router_responses_endpoints() {
		run(|host| async move {
			let open_router_model = with(
				&get_model("openai", "gpt-5.4"),
				json!({ "provider": "openrouter", "baseUrl": "https://openrouter.ai/api/v1" }),
			);
			let (captured, payload) = affinity(&host, "session-openrouter", open_router_model).await;

			assert_eq!(captured.session_id, None);
			assert_eq!(captured.client_request_id, None);
			assert_eq!(captured.x_session_id.as_deref(), Some("session-openrouter"));
			assert!(payload.get("session_id").is_none());
			assert_eq!(payload["prompt_cache_key"], "session-openrouter");
		});
	}

	#[test]
	fn uses_open_ai_no_session_format_when_configured() {
		run(|host| async move {
			let proxy_model = with(
				&get_model("openai", "gpt-5.4"),
				json!({
					"provider": "proxy",
					"baseUrl": "https://proxy.example.com/v1",
					"compat": { "sessionAffinityFormat": "openai-nosession" },
				}),
			);
			let (captured, payload) = affinity(&host, "session-proxy", proxy_model).await;

			assert_eq!(captured.session_id, None);
			assert_eq!(captured.client_request_id.as_deref(), Some("session-proxy"));
			assert_eq!(captured.x_session_id, None);
			assert!(payload.get("session_id").is_none());
			assert_eq!(payload["prompt_cache_key"], "session-proxy");
		});
	}

	#[test]
	fn uses_open_ai_no_session_format_for_open_code_responses_models() {
		run(|host| async move {
			let model = get_model("opencode", "gpt-5.4");
			let (captured, payload) = affinity(&host, "session-opencode", model.clone()).await;

			assert_eq!(
				model
					.compat
					.as_ref()
					.and_then(|compat| compat.get("sessionAffinityFormat")),
				Some(&json!("openai-nosession"))
			);
			assert_eq!(captured.session_id, None);
			assert_eq!(captured.client_request_id.as_deref(), Some("session-opencode"));
			assert_eq!(captured.x_session_id, None);
			assert_eq!(payload["prompt_cache_key"], "session-opencode");
		});
	}

	#[test]
	fn can_omit_open_ai_session_id_header_while_preserving_other_affinity_data() {
		run(|host| async move {
			let proxy_model = with(
				&get_model("openai", "gpt-5.4"),
				json!({
					"provider": "opencode",
					"baseUrl": "https://proxy.example.com/v1",
					"compat": { "sessionAffinityFormat": "openai-nosession" },
				}),
			);
			let (captured, payload) = affinity(&host, "session-123", proxy_model).await;

			assert_eq!(captured.session_id, None);
			assert_eq!(captured.client_request_id.as_deref(), Some("session-123"));
			assert_eq!(payload["prompt_cache_key"], "session-123");
		});
	}

	#[test]
	fn lets_explicit_headers_override_the_default_open_ai_cache_affinity_headers() {
		run(|host| async move {
			let captured = capture_open_ai_response_headers(
				&host,
				options_with(|options| {
					options.session_id = Some("session-123".into());
					let headers: ProviderHeaders = serde_json::from_value(json!({
						"session_id": "override-session",
						"x-client-request-id": "override-request",
					}))
					.expect("headers");
					options.headers = Some(headers);
				}),
				None,
			)
			.await;

			assert_eq!(captured.session_id.as_deref(), Some("override-session"));
			assert_eq!(captured.client_request_id.as_deref(), Some("override-request"));
		});
	}

	#[test]
	fn omits_open_ai_cache_affinity_headers_when_cache_retention_is_none() {
		run(|host| async move {
			let captured = capture_open_ai_response_headers(
				&host,
				options_with(|options| {
					options.cache_retention = Some(CacheRetention::None);
					options.session_id = Some("session-123".into());
				}),
				None,
			)
			.await;

			assert_eq!(captured.session_id, None);
			assert_eq!(captured.client_request_id, None);
		});
	}

	#[test]
	fn applies_cost_multiplier_for_requested_and_returned_service_tier() {
		let cases = [
			("gpt-5.4", "priority", "priority", 2.0),
			("gpt-5.5", "priority", "priority", 2.5),
			("gpt-5.5", "flex", "flex", 0.5),
			// GPT-6 models report Fast mode as "fast" even when "priority" is requested (#10034)
			("gpt-6-luna", "priority", "fast", 2.0),
			("gpt-6-luna", "fast", "fast", 2.0),
		];
		for (model_id, service_tier, response_service_tier, multiplier) in cases {
			run(|host| async move {
				let model = get_model("openai", model_id);
				let token_count = 100_000.0;
				let token_scale = token_count / 1_000_000.0;
				let event = json!({
					"type": "response.completed",
					"response": {
						"status": "completed",
						"service_tier": response_service_tier,
						"usage": {
							"input_tokens": token_count,
							"output_tokens": token_count,
							"total_tokens": token_count * 2.0,
							"input_tokens_details": { "cached_tokens": 0 },
						},
					},
				});
				respond_sse(&host, format!("data: {}\n\n", pi_host::js::json_stringify(&event)));

				let result = stream_open_ai_responses(
					&model,
					&sys_hi(),
					Some(options_with(|options| options.service_tier = Some(json!(service_tier)))),
				)
				.result()
				.await
				.expect("stream has a result");

				let case = format!("{model_id} {service_tier} {response_service_tier}");
				let close = |actual: f64, expected: f64| {
					assert!((actual - expected).abs() < 5e-13, "{case}: {actual} vs {expected}")
				};
				close(result.usage.cost.input, model.cost.input * multiplier * token_scale);
				close(result.usage.cost.output, model.cost.output * multiplier * token_scale);
				close(
					result.usage.cost.total,
					(model.cost.input + model.cost.output) * multiplier * token_scale,
				);
			});
		}
	}
}

mod openai_responses_max_output_tokens_compat {
	use super::*;

	#[test]
	fn sends_max_output_tokens_by_default() {
		run(|host| async move {
			respond_done(&host);
			let payload = capture(
				&get_model("openai", "gpt-5.4"),
				&sys_hi(),
				options_with(|options| options.max_tokens = Some(1024.0)),
			)
			.await;

			assert_eq!(payload["max_output_tokens"].as_f64(), Some(1024.0));
		});
	}

	#[test]
	fn omits_max_output_tokens_when_supports_max_output_tokens_is_false() {
		run(|host| async move {
			respond_done(&host);
			let base_model = get_model("openai", "gpt-5.4");
			let mut compat = base_model
				.compat
				.clone()
				.map(Value::Object)
				.unwrap_or_else(|| json!({}));
			compat["supportsMaxOutputTokens"] = json!(false);
			let model = with(&base_model, json!({ "compat": compat }));
			let payload = capture(
				&model,
				&sys_hi(),
				options_with(|options| options.max_tokens = Some(1024.0)),
			)
			.await;

			assert!(payload.get("max_output_tokens").is_none());
		});
	}
}
