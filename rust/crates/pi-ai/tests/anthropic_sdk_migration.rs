//! Port of `packages/ai/test/anthropic-sdk-migration.test.ts`.
//!
//! Differences from the TS:
//! - The real SDK becomes the Rust client (`api/anthropic_client.rs`); `new Anthropic({ apiKey, baseURL, fetch })` is
//!   `AnthropicClient::new` with the same options. `vi.stubEnv` is `TestHost::set_env`.
//! - An `it.each` table is one test fn named after the template; it runs every row in its own `TestHost` and a failing
//!   assertion names the row.
//! - `{ apiKey: "test-key", ...options }` is `key_options()`; a case with `apiKey: undefined` then clears the key.
//! - `Type.Object({})` is its JSON Schema `{ type: "object", properties: {} }`.

// The options' fields sit behind `Deref`, so the struct literal this lint suggests does not compile.
#![allow(clippy::field_reassign_with_default)]

mod anthropic_support;

use std::cell::RefCell;
use std::rc::Rc;

use indexmap::IndexMap;
use pi_ai::api::anthropic_client::{AnthropicClient, ClientOptions};
use pi_ai::api::anthropic_messages::{AnthropicOptions, stream, stream_simple};
use pi_ai::types::{Model, ProviderEnv, ProviderHeaders, SimpleStreamOptions, StopReason, TranscriptContext};
use pi_ai::utils::identity::format_user_agent;
use pi_host::testing::{RecordedRequest, run};
use pi_host::{FetchFunction, HttpResponse};
use serde_json::{Map, Value, json};

/// `RecordedRequest`: the request and its JSON body.
#[derive(Clone)]
struct Recorded {
	request: RecordedRequest,
	body: Value,
}

fn model() -> Model {
	serde_json::from_value(json!({
		"id": "claude-sonnet-4-6",
		"name": "Claude Sonnet 4.6",
		"api": "anthropic-messages",
		"provider": "anthropic",
		"baseUrl": "https://anthropic.test",
		"reasoning": true,
		"input": ["text"],
		"cost": { "input": 0, "output": 0, "cacheRead": 0, "cacheWrite": 0 },
		"contextWindow": 200000,
		"maxTokens": 4096,
	}))
	.unwrap()
}

/// `{ ...model, provider }`.
fn model_for(provider: &str) -> Model {
	let mut model = model();
	model.base.provider = provider.into();
	model
}

/// `{ ...target, headers }`.
fn with_headers(mut target: Model, headers: &[(&str, &str)]) -> Model {
	target.base.headers = Some(
		headers
			.iter()
			.map(|(name, value)| (name.to_string(), value.to_string()))
			.collect(),
	);
	target
}

fn context() -> TranscriptContext {
	anthropic_support::context(json!({
		"systemPrompt": "System prompt.",
		"messages": [{ "role": "user", "content": "Hello", "timestamp": 1 }],
	}))
}

fn headers(entries: &[(&str, Option<&str>)]) -> ProviderHeaders {
	entries
		.iter()
		.map(|(name, value)| (name.to_string(), value.map(str::to_string)))
		.collect()
}

/// `createFetch(requests)`.
fn create_fetch(requests: Rc<RefCell<Vec<Recorded>>>) -> FetchFunction {
	Rc::new(move |request| {
		assert_eq!(url::Url::parse(&request.url).unwrap().path(), "/v1/messages");
		let body_text = request
			.body
			.as_ref()
			.map(|body| String::from_utf8_lossy(body).into_owned())
			.unwrap_or_default();
		requests.borrow_mut().push(Recorded {
			body: serde_json::from_str(&body_text).unwrap(),
			request: RecordedRequest {
				method: request.method.clone(),
				url: request.url.clone(),
				headers: request.headers.clone(),
				body: Some(body_text),
			},
		});
		let events = [
			json!({
				"type": "message_start",
				"message": { "id": "msg_test", "model": "claude-sonnet-4-6", "usage": { "input_tokens": 1, "output_tokens": 0 } },
			}),
			json!({ "type": "message_delta", "delta": { "stop_reason": "end_turn" }, "usage": { "output_tokens": 1 } }),
			json!({ "type": "message_stop" }),
		];
		let body: String = events
			.iter()
			.map(|event| {
				format!(
					"event: {}\ndata: {}\n\n",
					event["type"].as_str().unwrap(),
					pi_host::js::json_stringify(event)
				)
			})
			.collect();
		Box::pin(async move {
			Ok(HttpResponse::from_bytes(
				200,
				IndexMap::from([("content-type".to_string(), "text/event-stream".to_string())]),
				body,
			))
		})
	})
}

/// `{ apiKey: "test-key", ...options }` without further options.
fn key_options() -> AnthropicOptions {
	let mut options = AnthropicOptions::default();
	options.api_key = Some("test-key".into());
	options
}

/// `{ apiKey: "test-key", temperature }`.
fn temperature_options(temperature: f64) -> AnthropicOptions {
	let mut options = key_options();
	options.temperature = Some(temperature);
	options
}

/// `captureRequest(target, options, transcript)`: `options` already holds the `apiKey` default.
async fn capture_request(target: &Model, mut options: AnthropicOptions, transcript: &TranscriptContext) -> Recorded {
	let requests: Rc<RefCell<Vec<Recorded>>> = Rc::default();
	options.fetch = Some(create_fetch(requests.clone()));
	let message = stream(target, transcript, Some(options)).result().await.unwrap();
	assert_eq!(message.stop_reason, StopReason::Stop, "{:?}", message.error_message);
	let requests = requests.borrow();
	assert_eq!(requests.len(), 1);
	requests[0].clone()
}

/// `expect(actual).toMatchObject(expected)`: objects match a subset of keys, arrays match element by element.
fn assert_match_object(actual: &Value, expected: &Value, path: &str) {
	match (actual, expected) {
		(Value::Object(actual), Value::Object(expected)) => {
			for (key, expected) in expected {
				let actual = actual.get(key).unwrap_or_else(|| panic!("{path}.{key} is missing"));
				assert_match_object(actual, expected, &format!("{path}.{key}"));
			}
		}
		(Value::Array(actual), Value::Array(expected)) => {
			assert_eq!(actual.len(), expected.len(), "{path} length");
			for (index, (actual, expected)) in actual.iter().zip(expected).enumerate() {
				assert_match_object(actual, expected, &format!("{path}[{index}]"));
			}
		}
		_ => assert_eq!(actual, expected, "{path}"),
	}
}

fn has_property(body: &Value, key: &str) -> bool {
	body.get(key).is_some()
}

mod anthropic_sdk_v1_wire_compatibility {
	use super::*;

	#[test]
	fn uses_the_native_version_and_bearer_auth_without_temperature() {
		for temperature in [0.0, 1.0] {
			run(|_| async move {
				let Recorded { request, body } =
					capture_request(&model(), temperature_options(temperature), &context()).await;

				assert_eq!(
					request.header("anthropic-version"),
					Some("2026-10-21"),
					"temperature={temperature}"
				);
				assert_eq!(request.header("authorization"), Some("Bearer test-key"));
				assert_eq!(request.header("x-api-key"), None);
				assert_eq!(request.header("user-agent"), Some(format_user_agent(None).as_str()));
				assert!(!has_property(&body, "temperature"));
				assert!(!has_property(&body, "top_p"));
				assert!(!has_property(&body, "top_k"));
			});
		}
	}

	#[test]
	fn preserves_the_legacy_version_x_api_key_and_supported_temperature() {
		for provider in ["kimi-coding", "minimax", "xiaomi", "openrouter", "vendor-proxy"] {
			run(|_| async move {
				let Recorded { request, body } =
					capture_request(&model_for(provider), temperature_options(0.0), &context()).await;

				assert_eq!(request.header("anthropic-version"), Some("2023-06-01"), "{provider}");
				assert_eq!(request.header("x-api-key"), Some("test-key"), "{provider}");
				assert_eq!(request.header("authorization"), None, "{provider}");
				assert_eq!(body["temperature"], json!(0), "{provider}");
			});
		}
	}

	#[test]
	fn keeps_copilot_bearer_auth_on_the_compatible_api_version() {
		run(|_| async {
			let Recorded { request, body } =
				capture_request(&model_for("github-copilot"), temperature_options(0.0), &context()).await;

			assert_eq!(request.header("anthropic-version"), Some("2023-06-01"));
			assert_eq!(request.header("authorization"), Some("Bearer test-key"));
			assert_eq!(request.header("x-api-key"), None);
			assert_eq!(request.header("x-initiator"), Some("user"));
			assert_eq!(body["temperature"], json!(0));
		});
	}

	#[test]
	fn uses_the_latest_claude_code_oauth_user_agent_instead_of_the_configured_identity() {
		run(|_| async {
			let mut options = temperature_options(0.0);
			options.api_key = Some("sk-ant-oat-test".into());
			options.identity =
				Some(serde_json::from_value(json!({ "userAgent": { "custom": "not-claude-code" } })).unwrap());
			let Recorded { request, body } = capture_request(&model(), options, &context()).await;

			assert_eq!(request.header("user-agent"), Some("claude-cli/2.1.307"));
			assert_eq!(request.header("authorization"), Some("Bearer sk-ant-oat-test"));
			assert_eq!(request.header("x-api-key"), None);
			assert_eq!(request.header("anthropic-version"), Some("2026-10-21"));
			assert!(request.header("anthropic-beta").unwrap().contains("oauth-2025-04-20"));
			assert_match_object(
				&body["system"],
				&json!([
					{ "text": "You are Claude Code, Anthropic's official CLI for Claude." },
					{ "text": "System prompt." },
				]),
				"system",
			);
			assert!(!has_property(&body, "temperature"));
		});
	}

	#[test]
	fn lets_mixed_case_caller_headers_override_model_version_auth_and_identity() {
		run(|_| async {
			let target = with_headers(
				model(),
				&[
					("Anthropic-Version", "2026-10-21"),
					("Authorization", "Bearer model-key"),
					("User-Agent", "model-client"),
				],
			);
			let mut options = temperature_options(0.0);
			options.headers = Some(headers(&[
				("ANTHROPIC-version", Some("2023-06-01")),
				("authorization", Some("Bearer caller-key")),
				("user-agent", Some("caller-client")),
			]));
			let Recorded { request, body } = capture_request(&target, options, &context()).await;

			assert_eq!(request.header("anthropic-version"), Some("2023-06-01"));
			assert_eq!(request.header("authorization"), Some("Bearer caller-key"));
			assert_eq!(request.header("user-agent"), Some("caller-client"));
			assert_eq!(body["temperature"], json!(0));
		});
	}

	#[test]
	fn omits_sampling_when_a_compatible_provider_explicitly_selects_the_native_version() {
		run(|_| async {
			let target = with_headers(model_for("vendor-proxy"), &[("Anthropic-Version", "2026-10-21")]);
			let Recorded { request, body } = capture_request(&target, temperature_options(0.0), &context()).await;

			assert_eq!(request.header("anthropic-version"), Some("2026-10-21"));
			assert_eq!(request.header("x-api-key"), Some("test-key"));
			assert_eq!(request.header("authorization"), None);
			assert!(!has_property(&body, "temperature"));
		});
	}

	#[test]
	fn honors_explicit_api_version_suppression_without_generating_legacy_sampling() {
		run(|_| async {
			let target = with_headers(model_for("vendor-proxy"), &[("Anthropic-Version", "2023-06-01")]);
			let mut options = temperature_options(0.0);
			options.headers = Some(headers(&[("anthropic-version", None)]));
			let Recorded { request, body } = capture_request(&target, options, &context()).await;

			assert_eq!(request.header("anthropic-version"), None);
			assert!(!has_property(&body, "temperature"));
		});
	}

	#[test]
	fn honors_authorization_null_suppression_after_a_model_owned_bearer_header() {
		run(|_| async {
			let target = with_headers(model(), &[("Authorization", "Bearer model-key")]);
			let mut options = key_options();
			options.headers = Some(headers(&[("authorization", None), ("X-Api-Key", Some("gateway-key"))]));
			let Recorded { request, .. } = capture_request(&target, options, &context()).await;

			assert_eq!(request.header("authorization"), None);
			assert_eq!(request.header("x-api-key"), Some("gateway-key"));
		});
	}

	#[test]
	fn allows_explicit_bearer_auth_on() {
		for provider in ["anthropic", "vendor-proxy"] {
			run(|_| async move {
				let mut options = key_options();
				options.api_key = None;
				options.headers = Some(headers(&[("Authorization", Some("Bearer gateway-token"))]));
				let Recorded { request, .. } = capture_request(&model_for(provider), options, &context()).await;

				assert_eq!(
					request.header("authorization"),
					Some("Bearer gateway-token"),
					"{provider}"
				);
				assert_eq!(request.header("x-api-key"), None, "{provider}");
			});
		}
	}

	#[test]
	fn does_not_resolve_ambient_sdk_credentials_for_model_owned_auth() {
		run(|host| async move {
			host.set_env("ANTHROPIC_API_KEY", "ambient-key");
			host.set_env("ANTHROPIC_AUTH_TOKEN", "ambient-token");
			host.set_env("ANTHROPIC_PROFILE", "missing-profile");
			host.set_env("ANTHROPIC_FEDERATION_RULE_ID", "fdrl_ambient");
			host.set_env("ANTHROPIC_ORGANIZATION_ID", "org_ambient");
			host.set_env("ANTHROPIC_IDENTITY_TOKEN_FILE", "/missing-identity.jwt");
			let target = with_headers(model(), &[("X-Api-Key", "model-key")]);
			let mut options = key_options();
			options.api_key = None;
			options.headers = Some(headers(&[("Authorization", None)]));
			let Recorded { request, .. } = capture_request(&target, options, &context()).await;

			assert_eq!(request.header("x-api-key"), Some("model-key"));
			assert_eq!(request.header("authorization"), None);
		});
	}

	#[test]
	fn accepts_model_owned_auth_through_stream_simple() {
		run(|_| async {
			let requests: Rc<RefCell<Vec<Recorded>>> = Rc::default();
			let mut options = SimpleStreamOptions::default();
			options.fetch = Some(create_fetch(requests.clone()));
			options.env = Some(ProviderEnv::new());
			let message = stream_simple(
				&with_headers(model(), &[("Authorization", "Bearer model-key")]),
				&context(),
				Some(options),
			)
			.unwrap()
			.result()
			.await
			.unwrap();

			assert_eq!(message.stop_reason, StopReason::Stop, "{:?}", message.error_message);
			let requests = requests.borrow();
			assert_eq!(requests.len(), 1);
			assert_eq!(requests[0].request.header("authorization"), Some("Bearer model-key"));
		});
	}

	#[test]
	fn preserves_injected_client_defaults_with_explicit_version() {
		for version in [None, Some("2023-06-01")] {
			run(|_| async move {
				let requests: Rc<RefCell<Vec<Recorded>>> = Rc::default();
				let client = AnthropicClient::new(ClientOptions {
					api_key: Some("injected-key".into()),
					base_url: Some(model().base_url.clone()),
					fetch: Some(create_fetch(requests.clone())),
					..ClientOptions::default()
				})
				.unwrap();
				let mut options = AnthropicOptions {
					client: Some(Rc::new(client)),
					..AnthropicOptions::default()
				};
				options.temperature = Some(0.0);
				if let Some(version) = version {
					options.headers = Some(headers(&[("Anthropic-Version", Some(version))]));
				}
				let message = stream(&model_for("google-vertex"), &context(), Some(options))
					.result()
					.await
					.unwrap();

				assert_eq!(message.stop_reason, StopReason::Stop, "{:?}", message.error_message);
				let requests = requests.borrow();
				assert_eq!(requests.len(), 1);
				let Recorded { request, body } = &requests[0];
				assert_eq!(
					request.header("authorization"),
					Some("Bearer injected-key"),
					"{version:?}"
				);
				assert_eq!(
					request.header("anthropic-version"),
					Some(version.unwrap_or("2026-10-21")),
					"{version:?}"
				);
				match version {
					None => assert!(!has_property(body, "temperature")),
					Some(_) => assert_eq!(body["temperature"], json!(0)),
				}
			});
		}
	}

	#[test]
	fn serializes_native_system_messages_and_tool_changes_with_role_specific_sdk_types() {
		run(|_| async {
			let empty_object = json!({ "type": "object", "properties": {} });
			let initial_tool = json!({ "name": "initial", "description": "Initial tool", "parameters": empty_object });
			let added_tool = json!({ "name": "added", "description": "Added tool", "parameters": empty_object });
			let mut target = model();
			target.compat = Some(Map::from_iter([
				("supportsMidConvoSystemMessages".to_string(), json!(true)),
				("supportsMidConvoToolChanges".to_string(), json!(true)),
			]));
			let transcript = anthropic_support::context(json!({
				"messages": [
					{ "role": "system", "content": "Initial prompt", "toolsAdded": [initial_tool], "timestamp": 0 },
					{ "role": "user", "content": "Hello", "timestamp": 1 },
					{
						"role": "system",
						"content": "Updated guidance",
						"toolsRemoved": [{ "name": "initial" }],
						"toolsAdded": [added_tool],
						"timestamp": 2,
					},
				],
			}));
			let Recorded { request, body } = capture_request(&target, temperature_options(0.0), &transcript).await;

			assert_eq!(request.header("anthropic-version"), Some("2026-10-21"));
			assert!(
				request
					.header("anthropic-beta")
					.unwrap()
					.contains("inline-tools-2026-09-15")
			);
			assert_match_object(
				&body["tools"],
				&json!([{ "name": "initial" }, { "defer_loading": true }]),
				"tools",
			);
			assert_match_object(
				body["messages"].as_array().unwrap().last().unwrap(),
				&json!({
					"role": "system",
					"content": [
						{ "type": "text", "text": "Updated guidance" },
						{ "type": "tool_removal", "tool": { "type": "tool_reference", "name": "initial" } },
						{
							"type": "tool_addition",
							"tool": { "type": "tool_definition", "definition": { "name": "added" } },
							"cache_control": { "type": "ephemeral" },
						},
					],
				}),
				"messages.at(-1)",
			);
			assert!(!has_property(&body, "temperature"));
		});
	}
}
