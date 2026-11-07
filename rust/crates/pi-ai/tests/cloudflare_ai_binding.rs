//! Port of `packages/ai/test/cloudflare-ai-binding.test.ts`.
//!
//! The fake binding is an [`AiBinding`] whose `fetch` records each request.

use std::cell::RefCell;
use std::rc::Rc;

use indexmap::IndexMap;
use pi_ai::api::cloudflare_ai_binding::{AiBinding, CLOUDFLARE_GATEWAY_BINDING_AUTH_SENTINEL, create_ai_binding_fetch};
use pi_ai::api::openai_completions;
use pi_ai::types::{
	Context, Message, Model, ProviderRequestOptions, SimpleStreamOptions, StopReason, StreamOptions, UserMessage,
};
use pi_ai::utils::transcript::normalize_context;
use pi_host::testing::run;
use pi_host::{FetchFunction, HttpRequest, HttpResponse};
use serde_json::json;

const BINDING_PREFIX: &str = "https://workers-binding.ai/ai-gateway/gateways/my-gateway";

fn fake_binding(response: impl Fn() -> HttpResponse + 'static) -> (AiBinding, Rc<RefCell<Vec<HttpRequest>>>) {
	let requests = Rc::new(RefCell::new(Vec::new()));
	let recorded = requests.clone();
	let fetch: FetchFunction = Rc::new(move |request: HttpRequest| {
		recorded.borrow_mut().push(request);
		let response = response();
		Box::pin(async move { Ok(response) })
	});
	(
		AiBinding {
			ai_gateway_log_id: None,
			fetch: Some(fetch),
		},
		requests,
	)
}

mod create_ai_binding_fetch {
	use super::*;

	#[test]
	fn passes_requests_to_the_binding_untouched() {
		run(|_host| async move {
			let (binding, requests) = fake_binding(|| {
				let mut headers = IndexMap::new();
				headers.insert("content-type".to_string(), "text/event-stream".to_string());
				headers.insert("cf-aig-log-id".to_string(), "log-1".to_string());
				HttpResponse::from_bytes(200, headers, "data: {}\n\n")
			});
			let fetch_fn = create_ai_binding_fetch(&binding).unwrap();
			// `expect(response).toBe(bindingResponse)`: the returned fetch is the binding's own, so its response comes back
			// unchanged.
			assert!(Rc::ptr_eq(&fetch_fn, binding.fetch.as_ref().unwrap()));
			let body = r#"{"model":"claude","messages":[{"role":"user","content":"hi"}]}"#;
			let mut request = HttpRequest::new("POST", format!("{BINDING_PREFIX}/anthropic/v1/messages?beta=true"));
			request.set_header("content-type", "application/json");
			request.set_header(
				"cf-aig-authorization",
				format!("Bearer {CLOUDFLARE_GATEWAY_BINDING_AUTH_SENTINEL}"),
			);
			request.set_header("anthropic-version", "2023-06-01");
			request.body = Some(body.into());

			let response = fetch_fn(request).await.unwrap();

			let requests = requests.borrow().clone();
			assert_eq!(
				requests[0].url,
				format!("{BINDING_PREFIX}/anthropic/v1/messages?beta=true")
			);
			assert_eq!(requests[0].method, "POST");
			let headers: Vec<(&str, &str)> = requests[0]
				.headers
				.iter()
				.map(|(name, value)| (name.as_str(), value.as_str()))
				.collect();
			// The gateway recognizes and strips the sentinel itself; nothing here touches headers.
			assert_eq!(
				headers,
				vec![
					("content-type", "application/json"),
					("cf-aig-authorization", "Bearer cloudflare-gateway-binding"),
					("anthropic-version", "2023-06-01"),
				]
			);
			assert_eq!(requests[0].body.as_deref(), Some(body.as_bytes()));
			assert_eq!(response.header("cf-aig-log-id"), Some("log-1"));
			assert_eq!(response.text().await.unwrap(), "data: {}\n\n");
		});
	}

	#[test]
	fn rejects_a_binding_with_no_fetch_at_construction_not_on_first_request() {
		// A binding on a runtime that predates `Ai#fetch`: the check happens early, with a message that names the cause.
		let Err(error) = create_ai_binding_fetch(&AiBinding::default()) else {
			panic!("expected an error");
		};
		assert!(error.to_string().contains("does not expose fetch()"), "{error}");
		assert_eq!(error.name, "TypeError");
	}

	#[test]
	fn keeps_sdk_placeholder_auth_off_the_wire_when_paired_with_null_auth_headers() {
		// The full header contract from the module docs, end to end through a real API impl: the
		// sentinel satisfies pi's request-auth check, and the explicit nulls make the OpenAI SDK
		// delete its own `Authorization: Bearer unused` placeholder before dispatch.
		run(|_host| async move {
			let (binding, requests) = fake_binding(|| {
				HttpResponse::from_bytes(
					400,
					IndexMap::from([("content-type".to_string(), "application/json".to_string())]),
					r#"{"error":{"type":"bad_request","message":"stubbed"}}"#,
				)
			});
			let model: Model = serde_json::from_value(json!({
				"id": "test-model",
				"name": "Test Model",
				"api": "openai-completions",
				"provider": "openai",
				"baseUrl": format!("{BINDING_PREFIX}/openai"),
				"reasoning": false,
				"input": ["text"],
				"cost": { "input": 0, "output": 0, "cacheRead": 0, "cacheWrite": 0 },
				"contextWindow": 10_000,
				"maxTokens": 1_000,
			}))
			.unwrap();
			let context = normalize_context(&Context {
				messages: vec![Message::User(
					serde_json::from_value::<UserMessage>(
						json!({ "role": "user", "content": "hello", "timestamp": 1 }),
					)
					.unwrap(),
				)],
				..Context::default()
			});
			let options = SimpleStreamOptions {
				base: StreamOptions {
					base: ProviderRequestOptions {
						headers: Some(IndexMap::from([
							(
								"cf-aig-authorization".to_string(),
								Some(format!("Bearer {CLOUDFLARE_GATEWAY_BINDING_AUTH_SENTINEL}")),
							),
							("Authorization".to_string(), None),
							("x-api-key".to_string(), None),
						])),
						fetch: Some(create_ai_binding_fetch(&binding).unwrap()),
						max_retries: Some(0.0),
						..ProviderRequestOptions::default()
					},
					..StreamOptions::default()
				},
				..SimpleStreamOptions::default()
			};

			let result = openai_completions::stream_simple(&model, &context, Some(options))
				.unwrap()
				.result()
				.await
				.unwrap();

			assert_eq!(result.stop_reason, StopReason::Error);
			let requests = requests.borrow();
			assert_eq!(requests.len(), 1);
			assert_eq!(requests[0].url, format!("{BINDING_PREFIX}/openai/chat/completions"));
			let header_names: Vec<String> = requests[0].headers.keys().map(|name| name.to_lowercase()).collect();
			assert!(!header_names.contains(&"authorization".to_string()), "{header_names:?}");
			assert!(!header_names.contains(&"x-api-key".to_string()), "{header_names:?}");
		});
	}
}
