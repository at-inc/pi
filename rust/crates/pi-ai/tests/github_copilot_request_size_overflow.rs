//! Port of `packages/ai/test/github-copilot-request-size-overflow.test.ts`.

mod openai_responses_support;

use std::cell::Cell;
use std::rc::Rc;

use openai_responses_support::{context, json_response, model, responses_options};
use pi_ai::api::openai_responses::stream;
use pi_ai::types::{Model, StopReason};
use pi_ai::utils::overflow::is_context_overflow;
use pi_host::FetchFunction;
use pi_host::testing::run;
use serde_json::json;

fn copilot_model() -> Model {
	model(json!({
		"id": "copilot-test-model",
		"name": "Copilot test model",
		"api": "openai-responses",
		"provider": "github-copilot",
		"baseUrl": "https://copilot.invalid",
		"reasoning": true,
		"input": ["text", "image"],
		"cost": { "input": 0, "output": 0, "cacheRead": 0, "cacheWrite": 0 },
		"contextWindow": 1_000_000,
		"maxTokens": 128_000,
	}))
}

mod copilot_http_request_size_errors {
	use super::*;

	#[test]
	fn classifies_http_status_with_nested_nested_through_the_real_sdk() {
		run(|_| async {
			for (status, nested, overflow) in [(413, true, true), (413, false, true), (400, true, false)] {
				let model = copilot_model();
				let error = json!({ "message": "failed to parse request", "code": "" });
				let body = if nested { json!({ "error": error }) } else { error };
				let calls = Rc::new(Cell::new(0));
				let counted = calls.clone();
				let fetch: FetchFunction = Rc::new(move |_| {
					counted.set(counted.get() + 1);
					let response = json_response(status, &body);
					Box::pin(async move { Ok(response) })
				});
				let options = responses_options(|options| {
					options.api_key = Some("test-key".into());
					options.fetch = Some(fetch);
					options.max_retries = Some(0.0);
				});
				let result = stream(
					&model,
					&context(json!({ "messages": [{ "role": "user", "content": "Continue", "timestamp": 0 }] })),
					Some(options),
				)
				.result()
				.await
				.expect("stream has a result");

				let case = format!("status {status} nested {nested}");
				assert_eq!(calls.get(), 1, "{case}");
				assert_eq!(result.provider, "github-copilot", "{case}");
				assert_eq!(result.stop_reason, StopReason::Error, "{case}");
				assert_eq!(result.usage.total_tokens, 0.0, "{case}");
				let error_message = result.error_message.clone().unwrap_or_default();
				assert!(
					error_message.contains(&format!("({status})")),
					"{case}: {error_message}"
				);
				assert!(
					error_message.contains("failed to parse request"),
					"{case}: {error_message}"
				);
				assert_eq!(
					is_context_overflow(&result, Some(model.context_window)),
					overflow,
					"{case}"
				);
			}
		});
	}
}
