//! Port of `packages/ai/test/anthropic-federation-sdk.test.ts`.
//!
//! Differences from the TS:
//! - The identity token file is written to the `TestHost` file system instead of a temp directory, and
//!   `vi.stubEnv` is `TestHost::set_env`.
//! - The recorded `path` is the URL path without the query, like `new URL(request.url).pathname`.

// The options' fields sit behind `Deref`, so the struct literal this lint suggests does not compile.
#![allow(clippy::field_reassign_with_default)]

mod anthropic_support;

use std::cell::RefCell;
use std::rc::Rc;

use anthropic_support::{SseEvent, event, sse_response};
use indexmap::IndexMap;
use pi_ai::api::anthropic_messages::{AnthropicOptions, stream};
use pi_ai::types::{Model, ProviderEnv, ProviderHeaders, StopReason, TranscriptContext};
use pi_host::testing::{TestHost, run};
use pi_host::{FetchFunction, HttpResponse};
use serde_json::json;

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

/// `RecordedRequest`: the URL path and the `authorization` header.
#[derive(Clone, Debug, PartialEq)]
struct RecordedRequest {
	path: String,
	authorization: Option<String>,
}

/// `createFetch(requests)`: answers the token exchange with `federated-token` and everything else with SSE.
fn create_fetch(requests: Rc<RefCell<Vec<RecordedRequest>>>) -> FetchFunction {
	Rc::new(move |request| {
		let path = url::Url::parse(&request.url).unwrap().path().to_string();
		requests.borrow_mut().push(RecordedRequest {
			path: path.clone(),
			authorization: request.header("authorization").map(str::to_string),
		});
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
	})
}

fn context() -> TranscriptContext {
	anthropic_support::context(json!({
		"systemPrompt": "System prompt.",
		"messages": [{ "role": "user", "content": "Hello", "timestamp": 1 }],
	}))
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

const IDENTITY_TOKEN_FILE: &str = "/tmp/pi-anthropic-federation-test/identity.jwt";

/// `beforeAll`: writes the identity token file and returns `federationEnv`.
fn federation_env(host: &TestHost) -> ProviderEnv {
	host.write_file(IDENTITY_TOKEN_FILE, "header.payload.signature");
	ProviderEnv::from([
		("ANTHROPIC_FEDERATION_RULE_ID".to_string(), "fdrl_test".to_string()),
		("ANTHROPIC_ORGANIZATION_ID".to_string(), "org-test".to_string()),
		(
			"ANTHROPIC_IDENTITY_TOKEN_FILE".to_string(),
			IDENTITY_TOKEN_FILE.to_string(),
		),
	])
}

// https://github.com/earendil-works/pi/issues/10177
mod anthropic_workload_identity_federation_with_the_sdk {
	use super::*;

	#[test]
	fn exchanges_the_identity_token_once_across_requests() {
		run(|host| async move {
			let federation_env = federation_env(&host);
			let requests: Rc<RefCell<Vec<RecordedRequest>>> = Rc::default();
			let fetch = create_fetch(requests.clone());

			for _ in 0..3 {
				let mut options = AnthropicOptions::default();
				options.env = Some(federation_env.clone());
				options.fetch = Some(fetch.clone());
				let message = stream(&anthropic_model(), &context(), Some(options))
					.result()
					.await
					.unwrap();
				assert_eq!(message.stop_reason, StopReason::Stop);
			}

			let requests = requests.borrow();
			assert_eq!(
				requests
					.iter()
					.filter(|request| request.path == "/v1/oauth/token")
					.count(),
				1
			);
			let message_requests: Vec<&RecordedRequest> = requests
				.iter()
				.filter(|request| request.path == "/v1/messages")
				.collect();
			assert_eq!(message_requests.len(), 3);
			for request in message_requests {
				assert_eq!(request.authorization.as_deref(), Some("Bearer federated-token"));
			}
		});
	}

	#[test]
	fn does_not_run_the_sdk_credential_chain_for_header_owned_auth() {
		run(|host| async move {
			for (name, value) in federation_env(&host) {
				host.set_env(&name, &value);
			}
			let requests: Rc<RefCell<Vec<RecordedRequest>>> = Rc::default();

			let mut options = AnthropicOptions::default();
			options.headers = Some(ProviderHeaders::from([(
				"Authorization".to_string(),
				Some("Bearer auth-token".to_string()),
			)]));
			options.fetch = Some(create_fetch(requests.clone()));
			let message = stream(&anthropic_model(), &context(), Some(options))
				.result()
				.await
				.unwrap();

			assert_eq!(message.stop_reason, StopReason::Stop);
			assert_eq!(
				*requests.borrow(),
				vec![RecordedRequest {
					path: "/v1/messages".into(),
					authorization: Some("Bearer auth-token".into()),
				}]
			);
		});
	}
}
