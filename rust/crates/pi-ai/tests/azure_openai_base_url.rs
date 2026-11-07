//! Port of `packages/ai/test/azure-openai-base-url.test.ts`.
//!
//! `vi.mock("openai")` captured the `AzureOpenAI` constructor options and the request params. Here the test host's
//! fetch records the request instead: the client `baseURL` is asserted as the URL the real SDK requests for it
//! (`<baseURL>/responses?api-version=v1`, recorded from the TS adapter with the real SDK), `defaultHeaders` as the
//! request headers, and the params as the request body. Environment variables come from the test host, which starts
//! with none set, like the `beforeEach` that deletes them.

mod openai_responses_support;

use std::cell::RefCell;
use std::rc::Rc;

use openai_responses_support::{context, response, sse_body, with, yield_now};
use pi_ai::api::azure_openai_responses::{AzureOpenAIResponsesOptions, stream as stream_azure_open_ai_responses};
use pi_ai::providers::all::get_builtin_model;
use pi_ai::types::{AssistantMessage, Model, OnProviderStreamEvent, StopReason, TranscriptContext};
use pi_ai::utils::identity::format_user_agent;
use pi_host::testing::{RecordedRequest, TestHost, run};
use serde_json::{Value, json};

fn azure_model() -> Model {
	get_builtin_model("azure", "gpt-4o-mini").expect("azure/gpt-4o-mini is defined")
}

fn hello_context() -> TranscriptContext {
	context(json!({ "messages": [{ "role": "user", "content": "hello", "timestamp": pi_host::now_ms() }] }))
}

/// The mocked `responses.create`: answers with `events` as SSE, or with an empty stream without them.
fn mock_azure(host: &TestHost, events: Option<Vec<Value>>) {
	let body = events.map_or_else(|| "data: [DONE]\n\n".to_string(), |events| sse_body(&events));
	host.on_fetch(move |_| {
		let body = body.clone();
		Box::pin(async move { Ok(response(200, &[("content-type", "text/event-stream")], body)) })
	});
}

async fn run_azure(
	host: &TestHost,
	model: &Model,
	context: &TranscriptContext,
	options: AzureOpenAIResponsesOptions,
) -> AssistantMessage {
	mock_azure(host, None);
	stream_azure_open_ai_responses(model, context, Some(options))
		.result()
		.await
		.expect("stream has a result")
}

fn options(configure: impl FnOnce(&mut AzureOpenAIResponsesOptions)) -> AzureOpenAIResponsesOptions {
	let mut options = AzureOpenAIResponsesOptions::default();
	configure(&mut options);
	options
}

fn only_request(host: &TestHost) -> RecordedRequest {
	let mut requests = host.requests();
	assert_eq!(requests.len(), 1);
	requests.remove(0)
}

async fn capture_client_url(host: &TestHost, base_url: &str) -> String {
	host.set_env("AZURE_OPENAI_BASE_URL", base_url);
	run_azure(
		host,
		&azure_model(),
		&hello_context(),
		options(|options| options.api_key = Some("test-api-key".into())),
	)
	.await;
	only_request(host).url
}

async fn capture_client_headers(host: &TestHost, headers: Option<Value>) -> RecordedRequest {
	run_azure(
		host,
		&azure_model(),
		&hello_context(),
		options(|options| {
			options.api_key = Some("test-api-key".into());
			options.azure_base_url = Some("https://my-resource.openai.azure.com".into());
			options.headers = headers.map(|headers| serde_json::from_value(headers).expect("headers"));
		}),
	)
	.await;
	only_request(host)
}

mod azure_base_url_normalization {
	use super::*;

	#[test]
	fn normalizes_cognitive_services_root_endpoints_to_openai_v1() {
		run(|host| async move {
			let url = capture_client_url(&host, "https://marc-quicktests-resource.cognitiveservices.azure.com").await;
			assert_eq!(
				url,
				"https://marc-quicktests-resource.cognitiveservices.azure.com/openai/v1/responses?api-version=v1"
			);
		});
	}

	#[test]
	fn normalizes_microsoft_foundry_root_endpoints_to_openai_v1() {
		run(|host| async move {
			let url = capture_client_url(&host, "https://marc-quicktests-resource.ai.azure.com").await;
			assert_eq!(
				url,
				"https://marc-quicktests-resource.ai.azure.com/openai/v1/responses?api-version=v1"
			);
		});
	}

	#[test]
	fn normalizes_azure_open_ai_root_endpoints_to_openai_v1() {
		run(|host| async move {
			let url = capture_client_url(&host, "https://my-resource.openai.azure.com").await;
			assert_eq!(
				url,
				"https://my-resource.openai.azure.com/openai/v1/responses?api-version=v1"
			);
		});
	}

	#[test]
	fn normalizes_openai_to_openai_v1() {
		run(|host| async move {
			let url = capture_client_url(&host, "https://my-resource.cognitiveservices.azure.com/openai").await;
			assert_eq!(
				url,
				"https://my-resource.cognitiveservices.azure.com/openai/v1/responses?api-version=v1"
			);
		});
	}

	#[test]
	fn preserves_openai_v1_endpoints() {
		run(|host| async move {
			let url = capture_client_url(&host, "https://my-resource.cognitiveservices.azure.com/openai/v1").await;
			assert_eq!(
				url,
				"https://my-resource.cognitiveservices.azure.com/openai/v1/responses?api-version=v1"
			);
		});
	}

	#[test]
	fn normalizes_openai_v1_responses_to_openai_v1() {
		run(|host| async move {
			let url = capture_client_url(&host, "https://my-resource.services.ai.azure.com/openai/v1/responses").await;
			assert_eq!(
				url,
				"https://my-resource.services.ai.azure.com/openai/v1/responses?api-version=v1"
			);
		});
	}

	#[test]
	fn preserves_explicit_non_azure_proxy_paths() {
		run(|host| async move {
			let url = capture_client_url(&host, "https://my-proxy.example.com/v1").await;
			assert_eq!(url, "https://my-proxy.example.com/v1/responses?api-version=v1");
		});
	}

	#[test]
	fn strips_query_params_when_normalizing_azure_host_urls() {
		run(|host| async move {
			let url = capture_client_url(
				&host,
				"https://my-resource.openai.azure.com/openai?api-version=2024-12-01",
			)
			.await;
			assert_eq!(
				url,
				"https://my-resource.openai.azure.com/openai/v1/responses?api-version=v1"
			);
		});
	}

	#[test]
	fn preserves_query_params_on_non_azure_proxy_urls() {
		run(|host| async move {
			let url = capture_client_url(&host, "https://my-proxy.example.com/v1?custom=true").await;
			// The SDK appends the path to the whole baseURL string, query included.
			assert_eq!(
				url,
				"https://my-proxy.example.com/v1?custom=true%2Fresponses&api-version=v1"
			);
		});
	}

	#[test]
	fn throws_on_invalid_urls() {
		run(|host| async move {
			host.set_env("AZURE_OPENAI_BASE_URL", "not-a-url");
			let result = run_azure(
				&host,
				&azure_model(),
				&hello_context(),
				options(|options| options.api_key = Some("test-api-key".into())),
			)
			.await;
			assert_eq!(result.stop_reason, StopReason::Error);
			let error_message = result.error_message.unwrap_or_default();
			assert!(
				error_message.contains("Invalid Azure OpenAI base URL"),
				"{error_message}"
			);
		});
	}

	#[test]
	fn clamps_prompt_cache_key_to_open_ai_s_64_character_limit() {
		run(|host| async move {
			run_azure(
				&host,
				&azure_model(),
				&hello_context(),
				options(|options| {
					options.api_key = Some("test-api-key".into());
					options.azure_base_url = Some("https://my-resource.openai.azure.com".into());
					options.session_id = Some("x".repeat(67));
				}),
			)
			.await;

			assert_eq!(only_request(&host).json()["prompt_cache_key"], json!("x".repeat(64)));
		});
	}

	#[test]
	fn disables_server_side_response_storage() {
		run(|host| async move {
			run_azure(
				&host,
				&azure_model(),
				&hello_context(),
				options(|options| {
					options.api_key = Some("test-api-key".into());
					options.azure_base_url = Some("https://my-resource.openai.azure.com".into());
				}),
			)
			.await;

			assert_eq!(only_request(&host).json()["store"], json!(false));
		});
	}

	#[test]
	fn honors_supports_strict_mode_false() {
		run(|host| async move {
			let base_model = azure_model();
			let mut compat = base_model
				.compat
				.clone()
				.map(Value::Object)
				.unwrap_or_else(|| json!({}));
			compat["supportsStrictMode"] = json!(false);
			let model = with(&base_model, json!({ "compat": compat }));
			let context = context(json!({
				"messages": [{ "role": "user", "content": "hello", "timestamp": pi_host::now_ms() }],
				"tools": [{
					"name": "preferred",
					"description": "Preferred constrained tool",
					"parameters": { "type": "object", "properties": { "value": { "type": "string" } }, "required": ["value"] },
					"constrainedSampling": { "type": "json_schema", "strict": "prefer" },
				}],
			}));

			run_azure(
				&host,
				&model,
				&context,
				options(|options| {
					options.api_key = Some("test-api-key".into());
					options.azure_base_url = Some("https://my-resource.openai.azure.com".into());
				}),
			)
			.await;

			assert!(only_request(&host).json()["tools"][0].get("strict").is_none());
		});
	}

	#[test]
	fn builds_correct_default_url_from_azure_openai_resource_name() {
		run(|host| async move {
			host.set_env("AZURE_OPENAI_RESOURCE_NAME", "my-resource");
			run_azure(
				&host,
				&azure_model(),
				&hello_context(),
				options(|options| options.api_key = Some("test-api-key".into())),
			)
			.await;
			assert_eq!(
				only_request(&host).url,
				"https://my-resource.openai.azure.com/openai/v1/responses?api-version=v1"
			);
		});
	}
}

mod azure_provider_stream_events {
	use super::*;

	#[test]
	fn forwards_parsed_events_in_order_before_normalizing_the_response() {
		run(|host| async move {
			let stream_events = vec![
				json!({ "type": "response.created", "sequence_number": 0, "response": { "id": "resp_azure" } }),
				json!({
					"type": "response.completed",
					"sequence_number": 1,
					"response": { "id": "resp_azure", "status": "completed" },
				}),
			];
			mock_azure(&host, Some(stream_events.clone()));
			let model = azure_model();
			let received: Rc<RefCell<Vec<Value>>> = Rc::default();
			let event_models: Rc<RefCell<Vec<Model>>> = Rc::default();
			let (received_target, models_target) = (received.clone(), event_models.clone());
			let on_provider_stream_event: OnProviderStreamEvent = Rc::new(move |event, event_model| {
				let (event, event_model) = (event.clone(), event_model.clone());
				let (received_target, models_target) = (received_target.clone(), models_target.clone());
				Box::pin(async move {
					yield_now().await;
					received_target.borrow_mut().push(event);
					models_target.borrow_mut().push(event_model);
					Ok(())
				})
			});
			let result = stream_azure_open_ai_responses(
				&model,
				&hello_context(),
				Some(options(|options| {
					options.api_key = Some("test-api-key".into());
					options.azure_base_url = Some("https://my-resource.openai.azure.com".into());
					options.on_provider_stream_event = Some(on_provider_stream_event);
				})),
			)
			.result()
			.await
			.expect("stream has a result");

			assert_eq!(*received.borrow(), stream_events);
			assert_eq!(*event_models.borrow(), [model.clone(), model]);
			assert_eq!(result.stop_reason, StopReason::Stop);
			assert_eq!(result.response_id.as_deref(), Some("resp_azure"));
		});
	}
}

mod azure_user_agent {
	use super::*;

	#[test]
	fn uses_pi_s_user_agent_by_default() {
		run(|host| async move {
			let request = capture_client_headers(&host, None).await;
			assert_eq!(request.header("user-agent"), Some(format_user_agent(None).as_str()));
		});
	}

	#[test]
	fn lets_explicit_headers_override_the_default_user_agent() {
		run(|host| async move {
			let request = capture_client_headers(&host, Some(json!({ "User-Agent": "custom-agent" }))).await;
			assert_eq!(request.header("user-agent"), Some("custom-agent"));
		});
	}
}
