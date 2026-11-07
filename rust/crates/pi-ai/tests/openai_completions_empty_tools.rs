//! Port of `packages/ai/test/openai-completions-empty-tools.test.ts`.
//!
//! The vitest file records the `create` params and the client options; the port reads the params from the request
//! body, `baseURL` from the request URL and `defaultHeaders` from the request headers. A `null` default header is one
//! the client does not send.
//!
//! Empty tools arrays must NOT be serialized as `tools: []`: some OpenAI-compatible backends (e.g. DashScope / Aliyun
//! Qwen via compatible-mode) reject the request with `"[] is too short - 'tools'"` (HTTP 400) when `--no-tools` produces
//! an empty array. Regression for https://github.com/earendil-works/pi/issues/3649

mod openai_completions_support;

use indexmap::IndexMap;
use openai_completions_support::{builtin_model, last_body, serve_chunks};
use pi_ai::compat::stream_simple;
use pi_ai::providers::all::get_builtin_model;
use pi_ai::types::{
	AssistantContent, AssistantMessage, Context, Message, Model, SimpleStreamOptions, StopReason, TextContent,
	ThinkingLevel, ToolCall, ToolResultMessage, UserContent, UserMessage, UserMessageContent,
};
use pi_host::testing::{RecordedRequest, TestHost, run};
use serde_json::{Value, json};

fn stop_chunk() -> Value {
	json!({
		"choices": [{ "delta": {}, "finish_reason": "stop" }],
		"usage": {
			"prompt_tokens": 1,
			"completion_tokens": 1,
			"prompt_tokens_details": { "cached_tokens": 0 },
			"completion_tokens_details": { "reasoning_tokens": 0 },
		},
	})
}

fn user(text: &str) -> Message {
	Message::User(UserMessage {
		content: UserMessageContent::Text(text.into()),
		timestamp: pi_host::now_ms(),
		..UserMessage::default()
	})
}

fn hi() -> Context {
	Context {
		messages: vec![user("hi")],
		..Context::default()
	}
}

fn api_key(key: &str) -> SimpleStreamOptions {
	let mut options = SimpleStreamOptions::default();
	options.base.api_key = Some(key.into());
	options
}

/// Streams `context` and returns the request the client sent.
async fn send(
	host: &TestHost,
	model: &Model,
	context: Context,
	options: Option<SimpleStreamOptions>,
) -> RecordedRequest {
	serve_chunks(host, vec![stop_chunk()]);
	stream_simple(model, &context, options).unwrap().result().await.unwrap();
	host.requests().last().cloned().expect("a request was sent")
}

fn set_cloudflare_env(host: &TestHost) {
	host.set_env("CLOUDFLARE_API_KEY", "cf-token");
	host.set_env("CLOUDFLARE_ACCOUNT_ID", "account-id");
	host.set_env("CLOUDFLARE_GATEWAY_ID", "gateway-id");
}

fn base_url(request: &RecordedRequest) -> &str {
	request
		.url
		.strip_suffix("/chat/completions")
		.expect("a chat completions URL")
}

mod openai_completions_empty_tools_handling {
	use super::*;

	#[test]
	fn omits_tools_field_when_context_tools_is_an_empty_array() {
		run(|host| async move {
			let context = Context {
				tools: Some(vec![]),
				..hi()
			};
			send(
				&host,
				&builtin_model("openai", "gpt-4o-mini"),
				context,
				Some(api_key("test")),
			)
			.await;
			assert!(!last_body(&host).as_object().unwrap().contains_key("tools"));
		});
	}

	#[test]
	fn omits_tools_field_when_context_tools_is_undefined() {
		run(|host| async move {
			send(
				&host,
				&builtin_model("openai", "gpt-4o-mini"),
				hi(),
				Some(api_key("test")),
			)
			.await;
			assert!(!last_body(&host).as_object().unwrap().contains_key("tools"));
		});
	}

	#[test]
	fn sends_default_max_tokens() {
		run(|host| async move {
			let model = builtin_model("openai", "gpt-4o-mini");
			send(&host, &model, hi(), Some(api_key("test"))).await;
			let params = last_body(&host);
			assert_eq!(params.get("max_tokens"), None);
			assert_eq!(params["max_completion_tokens"].as_f64(), Some(model.max_tokens));
		});
	}

	#[test]
	fn sends_explicit_max_tokens() {
		run(|host| async move {
			let mut options = api_key("test");
			options.base.max_tokens = Some(1234.0);
			send(&host, &builtin_model("openai", "gpt-4o-mini"), hi(), Some(options)).await;
			let params = last_body(&host);
			assert_eq!(params.get("max_tokens"), None);
			assert_eq!(params["max_completion_tokens"], json!(1234));
		});
	}

	fn clamped_model() -> Model {
		let mut model = builtin_model("openai", "gpt-4o-mini");
		model.context_window = 10000.0;
		model.max_tokens = 8000.0;
		model
	}

	fn long_context() -> Context {
		Context {
			messages: vec![user(&"x".repeat(8000))],
			..Context::default()
		}
	}

	#[test]
	fn clamps_default_max_tokens_to_remaining_context() {
		run(|host| async move {
			send(&host, &clamped_model(), long_context(), Some(api_key("test"))).await;
			let params = last_body(&host);
			assert_eq!(params.get("max_tokens"), None);
			assert_eq!(params["max_completion_tokens"], json!(3618));
		});
	}

	#[test]
	fn clamps_explicit_max_tokens_to_remaining_context() {
		run(|host| async move {
			let mut options = api_key("test");
			options.base.max_tokens = Some(7000.0);
			send(&host, &clamped_model(), long_context(), Some(options)).await;
			let params = last_body(&host);
			assert_eq!(params.get("max_tokens"), None);
			assert_eq!(params["max_completion_tokens"], json!(3618));
		});
	}

	#[test]
	fn uses_conservative_openai_compatible_fields_for_cloudflare_ai_gateway_compat_models() {
		run(|host| async move {
			set_cloudflare_env(&host);
			let model = get_builtin_model("cloudflare-ai-gateway", "workers-ai/@cf/moonshotai/kimi-k2.6").unwrap();
			let mut options = SimpleStreamOptions::default();
			options.base.max_tokens = Some(1234.0);
			options.reasoning = Some(ThinkingLevel::High);
			let context = Context {
				system_prompt: Some("You are helpful.".into()),
				..hi()
			};
			let request = send(&host, &model, context, Some(options)).await;

			let params = last_body(&host);
			assert_eq!(params["messages"][0]["role"], json!("system"));
			assert_eq!(params["max_tokens"], json!(1234));
			assert_eq!(params.get("max_completion_tokens"), None);
			assert_eq!(params.get("reasoning_effort"), None);
			assert_eq!(params.get("store"), None);

			assert_eq!(
				base_url(&request),
				"https://gateway.ai.cloudflare.com/v1/account-id/gateway-id/compat"
			);
			assert_eq!(request.header("Authorization"), None);
			assert_eq!(request.header("cf-aig-authorization"), Some("Bearer cf-token"));
		});
	}

	#[test]
	fn resolves_cloudflare_ai_gateway_base_url_through_provider_auth() {
		run(|host| async move {
			set_cloudflare_env(&host);
			let model = get_builtin_model("cloudflare-ai-gateway", "workers-ai/@cf/moonshotai/kimi-k2.6").unwrap();
			let request = send(&host, &model, hi(), None).await;
			assert_eq!(
				base_url(&request),
				"https://gateway.ai.cloudflare.com/v1/account-id/gateway-id/compat"
			);
		});
	}

	#[test]
	fn preserves_inline_upstream_authorization_for_cloudflare_ai_gateway_byok_requests() {
		run(|host| async move {
			set_cloudflare_env(&host);
			let model = get_builtin_model("cloudflare-ai-gateway", "gpt-5.1").unwrap();
			let mut options = SimpleStreamOptions::default();
			options.base.headers = Some(IndexMap::from([(
				"Authorization".to_string(),
				Some("Bearer upstream-token".to_string()),
			)]));
			let request = send(&host, &model, hi(), Some(options)).await;
			assert_eq!(request.header("Authorization"), Some("Bearer upstream-token"));
			assert_eq!(request.header("cf-aig-authorization"), Some("Bearer cf-token"));
		});
	}

	#[test]
	fn sends_session_affinity_headers_for_workers_ai_through_cloudflare_ai_gateway() {
		run(|host| async move {
			set_cloudflare_env(&host);
			let workers_model =
				get_builtin_model("cloudflare-ai-gateway", "workers-ai/@cf/moonshotai/kimi-k2.6").unwrap();
			let mut options = SimpleStreamOptions::default();
			options.base.session_id = Some("session-1".into());
			let request = send(&host, &workers_model, hi(), Some(options)).await;
			assert_eq!(request.header("session_id"), Some("session-1"));
			assert_eq!(request.header("x-client-request-id"), Some("session-1"));
			assert_eq!(request.header("x-session-affinity"), Some("session-1"));
		});
	}

	#[test]
	fn still_emits_tools_for_anthropic_litellm_proxy_when_conversation_has_tool_history() {
		run(|host| async move {
			let context = Context {
				messages: vec![
					user("use the tool"),
					Message::Assistant(AssistantMessage {
						content: vec![AssistantContent::ToolCall(ToolCall {
							id: "t1".into(),
							name: "noop".into(),
							..ToolCall::default()
						})],
						stop_reason: StopReason::ToolUse,
						api: "openai-completions".into(),
						provider: "openai".into(),
						model: "gpt-4o-mini".into(),
						timestamp: pi_host::now_ms(),
						..AssistantMessage::default()
					}),
					Message::ToolResult(ToolResultMessage {
						tool_call_id: "t1".into(),
						tool_name: "noop".into(),
						content: vec![UserContent::Text(TextContent {
							text: "done".into(),
							..TextContent::default()
						})],
						is_error: false,
						timestamp: pi_host::now_ms(),
						..ToolResultMessage::default()
					}),
				],
				tools: Some(vec![]),
				..Context::default()
			};
			send(
				&host,
				&builtin_model("openai", "gpt-4o-mini"),
				context,
				Some(api_key("test")),
			)
			.await;
			assert_eq!(last_body(&host)["tools"], json!([]));
		});
	}
}
