//! Port of `packages/ai/test/anthropic-federation.test.ts`.
//!
//! Differences from the TS:
//! - The TS mocks `@anthropic-ai/sdk` and records the client constructor options and the `beta.messages.create`
//!   params. Here the real client talks to the `TestHost` fetch, which answers the jwt-bearer token exchange
//!   (`/v1/oauth/token`) with an access token and every other request with the mocked SSE body. Constructor
//!   assertions become assertions on those requests: `config` is the token exchange the SDK performs for it (its body
//!   carries the federation rule, organization and service account ids, and the contents of the
//!   `identity_token` file as `assertion`; an `undefined` id is an absent key), `config: undefined` is no token
//!   exchange, `apiKey: null` is a messages request without `x-api-key`, `apiKey: "x"` is `authorization: Bearer x`
//!   (the native API version), and `defaultHeaders.Authorization` being undefined is the messages request carrying
//!   the exchanged token, which a default `Authorization` header would replace. A constructor that never ran is no
//!   request at all. The identity token file is written to the `TestHost` file system so the exchange can run.
//! - The `create` params are recorded through `onPayload`, which receives the params `create` is called with. The
//!   wire `anthropic-beta` header cannot stand in for them: the SDK appends `oauth-2025-04-20` to token-authenticated
//!   requests itself.

// The options' fields sit behind `Deref`, so the struct literal this lint suggests does not compile.
#![allow(clippy::field_reassign_with_default)]

mod anthropic_support;

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use anthropic_support::{SseEvent, event, sse_response};
use indexmap::IndexMap;
use pi_ai::api::anthropic_messages::{AnthropicOptions, stream};
use pi_ai::auth::types::{ApiKeyAuthInput, AuthContext, AuthResult, ModelAuth};
use pi_ai::models::{CreateModelsOptions, ModelsSimpleStreamOptions, create_models};
use pi_ai::providers::anthropic::anthropic_provider;
use pi_ai::types::{Context, Model, OnPayload, ProviderEnv, ProviderHeaders, StopReason, TranscriptContext};
use pi_ai::utils::transcript::normalize_context;
use pi_host::testing::{RecordedRequest, TestHost, run};
use pi_host::{AbortController, HttpResponse, LocalBoxFuture};
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

const IDENTITY_TOKEN: &str = "header.payload.signature";

/// Writes the identity token file and answers the token exchange with `federated-token` and every other request
/// with the mocked SSE response.
fn serve(host: &TestHost) {
	host.write_file("/tmp/identity.jwt", IDENTITY_TOKEN);
	host.on_fetch(|request| {
		let path = url::Url::parse(&request.url).unwrap().path().to_string();
		Box::pin(async move {
			if path == "/v1/oauth/token" {
				return Ok(HttpResponse::from_bytes(
					200,
					IndexMap::from([("content-type".to_string(), "application/json".to_string())]),
					r#"{"access_token":"federated-token","expires_in":3600}"#,
				));
			}
			Ok(sse_response(&sse_events()))
		})
	});
}

fn federation_env() -> Vec<(&'static str, &'static str)> {
	vec![
		("ANTHROPIC_FEDERATION_RULE_ID", "fdrl_test"),
		("ANTHROPIC_ORGANIZATION_ID", "org-test"),
		("ANTHROPIC_SERVICE_ACCOUNT_ID", "svac_test"),
		("ANTHROPIC_IDENTITY_TOKEN_FILE", "/tmp/identity.jwt"),
	]
}

/// `federationEnv` without `name`.
fn federation_env_without(name: &str) -> Vec<(&'static str, &'static str)> {
	federation_env().into_iter().filter(|(key, _)| *key != name).collect()
}

fn provider_env(env: &[(&str, &str)]) -> ProviderEnv {
	env.iter()
		.map(|(name, value)| (name.to_string(), value.to_string()))
		.collect()
}

/// The token exchange the SDK performs for `expectedConfig` with `service_account_id`.
fn expected_token_exchange(service_account_id: Option<&str>) -> Value {
	let mut body = json!({
		"grant_type": "urn:ietf:params:oauth:grant-type:jwt-bearer",
		"assertion": IDENTITY_TOKEN,
		"federation_rule_id": "fdrl_test",
		"organization_id": "org-test",
	});
	if let Some(service_account_id) = service_account_id {
		body["service_account_id"] = service_account_id.into();
	}
	body
}

/// The requests to `path`.
fn requests_to(host: &TestHost, path: &str) -> Vec<RecordedRequest> {
	host.requests()
		.into_iter()
		.filter(|request| url::Url::parse(&request.url).unwrap().path() == path)
		.collect()
}

/// `expect(mockState.constructorOpts?.config).toEqual(...)`: the one token exchange the client performed.
fn assert_token_exchange(host: &TestHost, expected: Value) {
	let exchanges = requests_to(host, "/v1/oauth/token");
	assert_eq!(exchanges.len(), 1, "one token exchange");
	assert_eq!(exchanges[0].url, "https://api.anthropic.com/v1/oauth/token");
	assert_eq!(exchanges[0].json(), expected);
}

/// The single messages request the client sent.
fn messages_request(host: &TestHost) -> RecordedRequest {
	let requests = requests_to(host, "/v1/messages");
	assert_eq!(requests.len(), 1, "one messages request");
	requests[0].clone()
}

fn context() -> TranscriptContext {
	normalize_context(&raw_context())
}

fn raw_context() -> Context {
	serde_json::from_value(json!({
		"systemPrompt": "System prompt.",
		"messages": [{ "role": "user", "content": "Hello", "timestamp": 1 }],
	}))
	.unwrap()
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

/// The stream options `{ env }`.
fn env_options(env: &[(&str, &str)]) -> AnthropicOptions {
	let mut options = AnthropicOptions::default();
	options.env = Some(provider_env(env));
	options
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

/// `resolveWithEnv(env)`.
async fn resolve_with_env(env: &[(&str, &str)]) -> Option<AuthResult> {
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

fn federation_result(env: &[(&str, &str)]) -> AuthResult {
	AuthResult {
		auth: ModelAuth::default(),
		env: Some(provider_env(env)),
		source: Some("workload identity federation".into()),
	}
}

// https://github.com/earendil-works/pi/issues/10177
mod anthropic_workload_identity_federation {
	use super::*;

	#[test]
	fn resolves_the_federation_variables_as_provider_env_with_no_request_auth() {
		run(|_| async {
			assert_eq!(
				resolve_with_env(&federation_env()).await,
				Some(federation_result(&federation_env()))
			);
		});
	}

	#[test]
	fn passes_anthropic_workspace_id_through_when_set() {
		run(|_| async {
			let mut env = federation_env();
			env.push(("ANTHROPIC_WORKSPACE_ID", "wrkspc_test"));
			let result = resolve_with_env(&env).await;
			assert_eq!(
				result
					.and_then(|result| result.env)
					.and_then(|env| env.get("ANTHROPIC_WORKSPACE_ID").cloned())
					.as_deref(),
				Some("wrkspc_test")
			);
		});
	}

	#[test]
	fn is_not_configured_when_a_federation_variable_is_missing() {
		run(|_| async {
			let partial = federation_env_without("ANTHROPIC_IDENTITY_TOKEN_FILE");
			assert_eq!(resolve_with_env(&partial).await, None);
		});
	}

	#[test]
	fn treats_anthropic_service_account_id_as_optional_like_the_sdk() {
		run(|host| async move {
			serve(&host);
			let partial = federation_env_without("ANTHROPIC_SERVICE_ACCOUNT_ID");
			assert_eq!(resolve_with_env(&partial).await, Some(federation_result(&partial)));

			stream(&anthropic_model(), &context(), Some(env_options(&partial)))
				.result()
				.await;
			assert_token_exchange(&host, expected_token_exchange(None));
		});
	}

	#[test]
	fn keeps_api_key_and_auth_token_precedence_over_federation() {
		run(|_| async {
			let mut with_api_key = federation_env();
			with_api_key.push(("ANTHROPIC_API_KEY", "api-key"));
			assert_eq!(
				resolve_with_env(&with_api_key).await,
				Some(AuthResult {
					auth: ModelAuth {
						api_key: Some("api-key".into()),
						..ModelAuth::default()
					},
					env: None,
					source: Some("ANTHROPIC_API_KEY".into()),
				})
			);
			let mut with_auth_token = federation_env();
			with_auth_token.push(("ANTHROPIC_AUTH_TOKEN", "auth-token"));
			assert_eq!(
				resolve_with_env(&with_auth_token).await,
				Some(AuthResult {
					auth: ModelAuth {
						headers: Some(ProviderHeaders::from([(
							"Authorization".to_string(),
							Some("Bearer auth-token".to_string()),
						)])),
						..ModelAuth::default()
					},
					env: None,
					source: Some("ANTHROPIC_AUTH_TOKEN".into()),
				})
			);
		});
	}

	#[test]
	fn hands_the_sdk_a_federation_config_instead_of_a_key() {
		run(|host| async move {
			serve(&host);
			let (on_payload, params) = record_params();
			let mut options = env_options(&federation_env());
			options.on_payload = Some(on_payload);
			stream(&anthropic_model(), &context(), Some(options)).result().await;

			let request = messages_request(&host);
			assert_eq!(request.header("x-api-key"), None);
			assert_token_exchange(&host, expected_token_exchange(Some("svac_test")));
			assert_eq!(request.header("authorization"), Some("Bearer federated-token"));
			let params = params.borrow().clone().expect("create was called");
			let betas = params.get("betas").cloned().unwrap_or(json!([]));
			assert!(!betas.as_array().unwrap().contains(&json!("oauth-2025-04-20")));
		});
	}

	#[test]
	fn threads_auth_context_federation_variables_through_models() {
		run(|host| async move {
			serve(&host);
			let models = create_models(Some(CreateModelsOptions {
				auth_context: Some(auth_context(&federation_env())),
				..CreateModelsOptions::default()
			}));
			models.set_provider(Rc::new(anthropic_provider()));

			models
				.stream_simple(&anthropic_model(), &raw_context(), None::<ModelsSimpleStreamOptions>)
				.result()
				.await;

			assert_eq!(messages_request(&host).header("x-api-key"), None);
			assert_token_exchange(&host, expected_token_exchange(Some("svac_test")));
		});
	}

	#[test]
	fn lets_an_explicit_api_key_win_over_federation_env() {
		run(|host| async move {
			serve(&host);
			let mut options = env_options(&federation_env());
			options.api_key = Some("explicit-key".into());
			stream(&anthropic_model(), &context(), Some(options)).result().await;

			assert_eq!(
				messages_request(&host).header("authorization"),
				Some("Bearer explicit-key")
			);
			assert!(requests_to(&host, "/v1/oauth/token").is_empty());
		});
	}

	#[test]
	fn does_not_federate_other_anthropic_messages_providers() {
		run(|host| async move {
			serve(&host);
			let mut kimi = anthropic_model();
			kimi.base.provider = "kimi-coding".into();
			kimi.base.base_url = "https://api.kimi.com/coding".into();
			let message = stream(&kimi, &context(), Some(env_options(&federation_env())))
				.result()
				.await
				.unwrap();

			assert_eq!(message.stop_reason, StopReason::Error);
			assert!(host.requests().is_empty());
		});
	}
}
