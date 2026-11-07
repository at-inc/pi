//! Port of `packages/ai/test/opencode-go-provider.test.ts`.
//!
//! The TS replaces `api/google-generative-ai.lazy.ts` with `vi.mock` and checks the options the mocked API received.
//! Rust has no module mocking, so `routes_through_google_generative_ai_with_the_open_code_session_header` lets the real
//! Google API send the request to the test host and checks what those options turn into on the wire: the model URL, the
//! `x-goog-api-key` header from `apiKey`, and the merged `x-custom` and `x-opencode-session` headers.
//! `routes_gemini_models_through_google_generative_ai` keeps the model lookup half on its own. `it.each` cases run in a
//! loop.

use indexmap::IndexMap;
use pi_ai::providers::opencode_go::opencode_go_provider;
use pi_ai::types::{Context, ProviderHeaders, ProviderStreamOptions, SimpleStreamOptions, StopReason, StreamOptions};
use pi_ai::utils::transcript::normalize_context;
use pi_host::HttpResponse;
use pi_host::js::json_stringify;
use pi_host::testing::run;
use serde_json::json;

mod open_code_go_gemini_dispatch {
	use super::*;

	#[test]
	fn routes_gemini_models_through_google_generative_ai() {
		let provider = opencode_go_provider();
		let model = (provider.get_models)()
			.into_iter()
			.find(|model| model.id == "gemini-4-argon")
			.expect("OpenCode Go Gemini model is missing");
		assert_eq!(model.api, "google-generative-ai");
		assert_eq!(model.provider, "opencode-go");
		assert_eq!(model.base_url, "https://opencode.ai/inference/go/google/v1beta");
	}

	#[test]
	fn routes_through_google_generative_ai_with_the_open_code_session_header() {
		for method in ["stream", "streamSimple"] {
			run(|host| async move {
				let chunk = json!({
					"candidates": [{ "content": { "role": "model", "parts": [{ "text": "ok" }] }, "finishReason": "STOP" }],
					"usageMetadata": { "promptTokenCount": 1, "candidatesTokenCount": 1, "totalTokenCount": 2 },
				});
				host.on_fetch(move |_| {
					let body = format!("data: {}\r\n\r\n", json_stringify(&chunk));
					Box::pin(async move {
						Ok(HttpResponse::from_bytes(
							200,
							IndexMap::from([("content-type".to_string(), "text/event-stream".to_string())]),
							body,
						))
					})
				});
				let provider = opencode_go_provider();
				let model = (provider.get_models)()
					.into_iter()
					.find(|model| model.id == "gemini-4-argon")
					.expect("OpenCode Go Gemini model is missing");
				let context = normalize_context(
					&serde_json::from_value::<Context>(
						json!({ "messages": [{ "role": "user", "content": "hello", "timestamp": 0 }] }),
					)
					.unwrap(),
				);
				let mut base = StreamOptions::default();
				base.api_key = Some("test-key".into());
				base.session_id = Some("conversation-1".into());
				base.headers = Some(ProviderHeaders::from([(
					"x-custom".to_string(),
					Some("value".to_string()),
				)]));
				let stream = match method {
					"stream" => (provider.stream)(
						&model,
						&context,
						Some(ProviderStreamOptions {
							base,
							..ProviderStreamOptions::default()
						}),
					),
					_ => (provider.stream_simple)(
						&model,
						&context,
						Some(SimpleStreamOptions {
							base,
							..SimpleStreamOptions::default()
						}),
					),
				};
				let result = stream.unwrap().result().await.expect("stream has a result");

				assert_eq!(
					result.stop_reason,
					StopReason::Stop,
					"{method}: {:?}",
					result.error_message
				);
				let requests = host.requests();
				assert_eq!(requests.len(), 1, "{method}");
				let request = &requests[0];
				assert!(
					request.url.starts_with(
						"https://opencode.ai/inference/go/google/v1beta/models/gemini-4-argon:streamGenerateContent"
					),
					"{method}: {}",
					request.url
				);
				assert_eq!(request.header("x-goog-api-key"), Some("test-key"), "{method}");
				assert_eq!(request.header("x-custom"), Some("value"), "{method}");
				assert_eq!(request.header("x-opencode-session"), Some("conversation-1"), "{method}");
			});
		}
	}
}
