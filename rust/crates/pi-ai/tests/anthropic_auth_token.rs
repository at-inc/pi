//! Port of `packages/ai/test/anthropic-auth-token.test.ts`.
//!
//! Differences from the TS:
//! - The TS mocks `@anthropic-ai/sdk` and records the client constructor options and the `beta.messages.create`
//!   params. Here the real client sends its request to the `TestHost` fetch, which answers with the same SSE body.
//!   Constructor assertions become assertions on that request: `apiKey: null` and `authToken: null` are a request
//!   without `x-api-key` whose `authorization` is exactly the caller's header, `authToken: "x"` is
//!   `authorization: Bearer x`, and `defaultHeaders.X` is the request header `X`.
//! - The `create` params are recorded through `onPayload`, which receives the params `create` is called with.

// The options' fields sit behind `Deref`, so the struct literal this lint suggests does not compile.
#![allow(clippy::field_reassign_with_default)]

mod anthropic_support;

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use anthropic_support::{SseEvent, event, sse_response};
use pi_ai::api::anthropic_messages::{AnthropicOptions, stream};
use pi_ai::auth::types::{ApiKeyAuthInput, AuthContext, AuthResult, ModelAuth};
use pi_ai::env_api_keys::{ANTHROPIC_AUTH_TOKEN_ENV, ANTHROPIC_OAUTH_TOKEN_ENV};
use pi_ai::models::{CreateModelsOptions, ModelsSimpleStreamOptions, MutableModels, create_models};
use pi_ai::providers::anthropic::anthropic_provider;
use pi_ai::types::{Context, Model, OnPayload, ProviderHeaders, TranscriptContext};
use pi_ai::utils::identity::format_user_agent;
use pi_ai::utils::transcript::normalize_context;
use pi_host::testing::{RecordedRequest, TestHost, run};
use pi_host::{AbortController, LocalBoxFuture};
use serde_json::{Value, json};

/// The mocked SDK's `createSseResponse()`.
fn sse_events() -> Vec<SseEvent> {
	vec![
		event(
			"message_start",
			json!({
				"type": "message_start",
				"message": { "id": "msg_test", "usage": { "input_tokens": 1, "output_tokens": 0 } },
			}),
		),
		event(
			"message_delta",
			json!({ "type": "message_delta", "delta": { "stop_reason": "end_turn" }, "usage": { "output_tokens": 1 } }),
		),
		event("message_stop", json!({ "type": "message_stop" })),
	]
}

/// Answers every request with the mocked SDK's SSE response.
fn serve_sse(host: &TestHost) {
	host.on_fetch(|_| Box::pin(async { Ok(sse_response(&sse_events())) }));
}

fn raw_context() -> Context {
	serde_json::from_value(json!({
		"systemPrompt": "System prompt.",
		"messages": [{ "role": "user", "content": "Hello", "timestamp": 1 }],
	}))
	.unwrap()
}

fn context() -> TranscriptContext {
	normalize_context(&raw_context())
}

fn anthropic_model() -> Model {
	serde_json::from_value(json!({
		"id": "claude-test",
		"name": "Claude Test",
		"api": "anthropic-messages",
		"provider": "anthropic",
		"baseUrl": "https://api.anthropic.com",
		"reasoning": false,
		"input": ["text"],
		"cost": { "input": 0, "output": 0, "cacheRead": 0, "cacheWrite": 0 },
		"contextWindow": 100000,
		"maxTokens": 4096,
	}))
	.unwrap()
}

fn kimi_coding_model() -> Model {
	let mut model = anthropic_model();
	model.base.id = "kimi-for-coding".into();
	model.base.name = "Kimi For Coding".into();
	model.base.provider = "kimi-coding".into();
	model.base.base_url = "https://api.kimi.com/coding".into();
	model
}

fn headers(entries: &[(&str, Option<&str>)]) -> ProviderHeaders {
	entries
		.iter()
		.map(|(name, value)| (name.to_string(), value.map(str::to_string)))
		.collect()
}

/// `mockState.createParams`: an `onPayload` that records the params and keeps them unchanged.
fn record_params() -> (OnPayload, Rc<RefCell<Option<Value>>>) {
	let params: Rc<RefCell<Option<Value>>> = Rc::default();
	let recorded = params.clone();
	let on_payload: OnPayload = Rc::new(move |payload, _| {
		*recorded.borrow_mut() = Some(payload);
		Box::pin(async { Ok(None) })
	});
	(on_payload, params)
}

/// The single `<base>/v1/messages` request the client sent.
fn messages_request(host: &TestHost) -> RecordedRequest {
	let requests: Vec<RecordedRequest> = host
		.requests()
		.into_iter()
		.filter(|request| url::Url::parse(&request.url).unwrap().path().ends_with("/v1/messages"))
		.collect();
	assert_eq!(requests.len(), 1, "one messages request");
	requests[0].clone()
}

/// `params.betas ?? []`.
fn betas(params: &Value) -> Vec<String> {
	params
		.get("betas")
		.and_then(Value::as_array)
		.map(|betas| betas.iter().map(|beta| beta.as_str().unwrap().to_string()).collect())
		.unwrap_or_default()
}

/// `expect(params.system).toEqual([expect.objectContaining({ text: "System prompt." })])`.
fn assert_single_system_prompt(params: &Value) {
	let system = params["system"].as_array().expect("system is an array");
	assert_eq!(system.len(), 1);
	assert_eq!(system[0]["text"], "System prompt.");
}

struct FakeAuthContext {
	env: HashMap<String, String>,
}

impl AuthContext for FakeAuthContext {
	fn env(&self, name: &str) -> LocalBoxFuture<'static, Option<String>> {
		let value = self.env.get(name).cloned();
		Box::pin(async move { value })
	}

	fn file_exists(&self, _path: &str) -> LocalBoxFuture<'static, bool> {
		Box::pin(async { false })
	}
}

fn auth_context(env: &[(&str, &str)]) -> Rc<dyn AuthContext> {
	Rc::new(FakeAuthContext {
		env: env
			.iter()
			.map(|(name, value)| (name.to_string(), value.to_string()))
			.collect(),
	})
}

/// `anthropicProvider().auth.apiKey?.resolve({ ctx, signal: neverAbortedSignal })`.
async fn resolve(env: &[(&str, &str)]) -> Option<AuthResult> {
	let provider = anthropic_provider();
	let resolve = provider.auth.api_key.as_ref().expect("api key auth").resolve.clone();
	resolve(ApiKeyAuthInput {
		ctx: auth_context(env),
		credential: None,
		signal: AbortController::new().signal(),
	})
	.await
	.unwrap()
}

/// `createModels({ authContext })` with the anthropic provider.
fn models_with_env(env: &[(&str, &str)]) -> Rc<dyn MutableModels> {
	let models = create_models(Some(CreateModelsOptions {
		auth_context: Some(auth_context(env)),
		..CreateModelsOptions::default()
	}));
	models.set_provider(Rc::new(anthropic_provider()));
	models
}

/// `createParams.betas ?? []` contains `oauth-2025-04-20`.
fn has_oauth_beta(params: &RefCell<Option<Value>>) -> bool {
	let params = params.borrow().clone().expect("create was called");
	betas(&params).iter().any(|beta| beta == "oauth-2025-04-20")
}

mod anthropic_auth_token_env {
	use super::*;

	#[test]
	fn resolves_anthropic_auth_token_as_a_bearer_authorization_header() {
		run(|_| async {
			let auth = resolve(&[
				("ANTHROPIC_AUTH_TOKEN", "auth-token"),
				("ANTHROPIC_OAUTH_TOKEN", "oauth-token"),
				("ANTHROPIC_API_KEY", "api-key"),
			])
			.await;

			assert_eq!(
				auth,
				Some(AuthResult {
					auth: ModelAuth {
						headers: Some(headers(&[("Authorization", Some("Bearer auth-token"))])),
						..ModelAuth::default()
					},
					env: None,
					source: Some(ANTHROPIC_AUTH_TOKEN_ENV.into()),
				})
			);
		});
	}

	#[test]
	fn preserves_anthropic_oauth_token_as_oauth_shaped_api_auth() {
		run(|_| async {
			let auth = resolve(&[
				("ANTHROPIC_OAUTH_TOKEN", "oauth-token"),
				("ANTHROPIC_API_KEY", "api-key"),
			])
			.await;

			assert_eq!(
				auth,
				Some(AuthResult {
					auth: ModelAuth {
						api_key: Some("oauth-token".into()),
						..ModelAuth::default()
					},
					env: None,
					source: Some(ANTHROPIC_OAUTH_TOKEN_ENV.into()),
				})
			);
		});
	}

	#[test]
	fn uses_authorization_headers_without_oauth_mode_request_shaping() {
		run(|host| async move {
			serve_sse(&host);
			let (on_payload, params) = record_params();
			let mut options = AnthropicOptions::default();
			options.headers = Some(headers(&[("Authorization", Some("Bearer gateway-token"))]));
			options.on_payload = Some(on_payload);
			stream(&anthropic_model(), &context(), Some(options)).result().await;

			let request = messages_request(&host);
			assert_eq!(request.header("x-api-key"), None);
			assert_eq!(request.header("authorization"), Some("Bearer gateway-token"));
			assert!(!has_oauth_beta(&params));
			assert_single_system_prompt(params.borrow().as_ref().unwrap());
		});
	}

	#[test]
	fn threads_auth_context_anthropic_auth_token_through_request_headers() {
		run(|host| async move {
			serve_sse(&host);
			let models = models_with_env(&[("ANTHROPIC_AUTH_TOKEN", "ctx-token")]);
			let (on_payload, params) = record_params();
			let mut options = ModelsSimpleStreamOptions::default();
			options.on_payload = Some(on_payload);

			models
				.stream_simple(&anthropic_model(), &raw_context(), Some(options))
				.result()
				.await;

			let request = messages_request(&host);
			assert_eq!(request.header("x-api-key"), None);
			assert_eq!(request.header("authorization"), Some("Bearer ctx-token"));
			assert!(!has_oauth_beta(&params));
			assert_single_system_prompt(params.borrow().as_ref().unwrap());
		});
	}

	#[test]
	fn preserves_oauth_request_shaping_for_anthropic_oauth_token() {
		run(|host| async move {
			serve_sse(&host);
			let models = models_with_env(&[("ANTHROPIC_OAUTH_TOKEN", "sk-ant-oat-test")]);
			let (on_payload, params) = record_params();
			let mut options = ModelsSimpleStreamOptions::default();
			options.on_payload = Some(on_payload);

			models
				.stream_simple(&anthropic_model(), &raw_context(), Some(options))
				.result()
				.await;

			let request = messages_request(&host);
			assert_eq!(request.header("x-api-key"), None);
			assert_eq!(request.header("authorization"), Some("Bearer sk-ant-oat-test"));
			assert!(has_oauth_beta(&params));
		});
	}

	#[test]
	fn lets_explicit_request_headers_override_anthropic_auth_token() {
		run(|host| async move {
			serve_sse(&host);
			let models = models_with_env(&[("ANTHROPIC_AUTH_TOKEN", "ctx-token")]);
			let mut options = ModelsSimpleStreamOptions::default();
			options.headers = Some(headers(&[("Authorization", Some("Bearer explicit-token"))]));

			models
				.stream_simple(&anthropic_model(), &raw_context(), Some(options))
				.result()
				.await;

			assert_eq!(
				messages_request(&host).header("authorization"),
				Some("Bearer explicit-token")
			);
		});
	}
}

mod anthropic_compatible_user_agents {
	use super::*;

	#[test]
	fn uses_pis_user_agent_by_default_for_anthropic_messages_requests() {
		run(|host| async move {
			serve_sse(&host);
			let mut options = AnthropicOptions::default();
			options.api_key = Some("anthropic-key".into());
			stream(&anthropic_model(), &context(), Some(options)).result().await;

			assert_eq!(
				messages_request(&host).header("user-agent"),
				Some(format_user_agent(None).as_str())
			);
		});
	}

	#[test]
	fn lets_explicit_headers_override_the_default_anthropic_messages_user_agent() {
		run(|host| async move {
			serve_sse(&host);
			let mut options = AnthropicOptions::default();
			options.api_key = Some("kimi-key".into());
			options.headers = Some(headers(&[("User-Agent", Some("custom-client"))]));
			stream(&kimi_coding_model(), &context(), Some(options)).result().await;

			assert_eq!(messages_request(&host).header("user-agent"), Some("custom-client"));
		});
	}

	#[test]
	fn preserves_explicit_anthropic_beta_header_replacement() {
		run(|host| async move {
			serve_sse(&host);
			let (on_payload, params) = record_params();
			let mut options = AnthropicOptions::default();
			options.api_key = Some("anthropic-key".into());
			options.headers = Some(headers(&[("anthropic-beta", Some("custom-beta"))]));
			options.on_payload = Some(on_payload);
			stream(&anthropic_model(), &context(), Some(options)).result().await;

			let params = params.borrow().clone().expect("create was called");
			assert_eq!(params["betas"], json!(["custom-beta"]));
		});
	}

	#[test]
	fn preserves_explicit_anthropic_beta_header_suppression() {
		run(|host| async move {
			serve_sse(&host);
			let (on_payload, params) = record_params();
			let mut options = AnthropicOptions::default();
			options.api_key = Some("anthropic-key".into());
			options.headers = Some(headers(&[("anthropic-beta", None)]));
			options.on_payload = Some(on_payload);
			stream(&anthropic_model(), &context(), Some(options)).result().await;

			let params = params.borrow().clone().expect("create was called");
			assert_eq!(params.get("betas"), None);
		});
	}
}
