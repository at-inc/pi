//! Port of `packages/ai/test/mistral-raw-stop-reason.test.ts`.

use std::rc::Rc;

use futures::StreamExt;
use indexmap::IndexMap;
use pi_ai::api::mistral_conversations::{MistralOptions, stream};
use pi_ai::providers::all::get_builtin_model;
use pi_ai::types::{Context, Model, ProviderRequestOptions, StopReason, StreamOptions, TranscriptContext};
use pi_ai::utils::retry::is_retryable_assistant_error;
use pi_ai::utils::transcript::normalize_context;
use pi_host::js::json_stringify;
use pi_host::testing::run;
use pi_host::{FetchFunction, HttpResponse};
use serde_json::json;

fn model() -> Model {
	get_builtin_model("mistral", "devstral-medium-latest").expect("built-in model")
}

fn context() -> TranscriptContext {
	normalize_context(
		&serde_json::from_value::<Context>(json!({
			"messages": [{ "role": "user", "content": "hello", "timestamp": 1 }],
		}))
		.expect("valid context"),
	)
}

/// `createFetch(finishReason)`.
fn create_fetch(finish_reason: &'static str) -> FetchFunction {
	Rc::new(move |_request| {
		let event = json!({
			"id": "mistral-response-id",
			"model": model().id,
			"choices": [{ "index": 0, "finish_reason": finish_reason, "delta": {} }],
			"usage": { "prompt_tokens": 1, "completion_tokens": 0, "total_tokens": 1 },
		});
		let body = format!("data: {}\n\ndata: [DONE]\n\n", json_stringify(&event));
		let mut response = HttpResponse::from_bytes(
			200,
			IndexMap::from([("content-type".to_string(), "text/event-stream".to_string())]),
			Vec::new(),
		);
		response.body = futures::stream::once(async move { Ok(body.into()) }).boxed_local();
		Box::pin(async move { Ok(response) })
	})
}

async fn result(finish_reason: &'static str) -> pi_ai::types::AssistantMessage {
	let options = MistralOptions {
		base: StreamOptions {
			base: ProviderRequestOptions {
				api_key: Some("test".into()),
				fetch: Some(create_fetch(finish_reason)),
				..ProviderRequestOptions::default()
			},
			..StreamOptions::default()
		},
		..MistralOptions::default()
	};
	stream(&model(), &context(), Some(options))
		.result()
		.await
		.expect("stream result")
}

mod mistral_raw_stop_reasons {
	use super::*;

	#[test]
	fn preserves_raw_mistral_finish_reasons_for_successful_stops() {
		run(|_host| async move {
			let message = result("stop").await;

			assert_eq!(message.stop_reason, StopReason::Stop);
			assert_eq!(message.raw_stop_reason.as_deref(), Some("stop"));
			assert_eq!(message.error_message, None);
		});
	}

	#[test]
	fn preserves_raw_mistral_finish_reasons_for_provider_error_stops() {
		run(|_host| async move {
			let message = result("error").await;

			assert_eq!(message.stop_reason, StopReason::Error);
			assert_eq!(message.raw_stop_reason.as_deref(), Some("error"));
			assert_eq!(
				message.error_message.as_deref(),
				Some("Provider stopped with: error (server error)")
			);
			// #10487
			assert!(is_retryable_assistant_error(&message));
		});
	}

	#[test]
	fn treats_unknown_mistral_finish_reasons_as_provider_error_stops() {
		run(|_host| async move {
			let message = result("unmapped_error").await;

			assert_eq!(message.stop_reason, StopReason::Error);
			assert_eq!(message.raw_stop_reason.as_deref(), Some("unmapped_error"));
			assert_eq!(
				message.error_message.as_deref(),
				Some("Provider stopped with: unmapped_error")
			);
			assert!(!is_retryable_assistant_error(&message));
		});
	}
}
