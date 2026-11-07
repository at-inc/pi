//! Port of `packages/ai/test/provider-error-body-regression.test.ts`: the openai-responses case.
//!
//! `vi.mock("openai")` threw an `APIError(403)` carrying the parsed body; here the test host answers the request with
//! that 403 and body, and the real client builds the error. The openai-completions cases (w3-openai-completions) and
//! the Bedrock cases (w3-bedrock-misc) move with those adapters, which are still `PORT-STATUS: pending` here.

mod openai_responses_support;

use openai_responses_support::{context, json_response, model, responses_options};
use pi_ai::api::openai_responses::stream as stream_open_ai_responses;
use pi_ai::types::StopReason;
use pi_host::testing::run;
use serde_json::json;

mod provider_error_body_passthrough_per_tier_regression {
	use super::*;

	#[test]
	fn openai_responses_status_only_keeps_the_prefix_and_surfaces_the_body() {
		run(|host| async move {
			host.on_fetch(|_| {
				Box::pin(async { Ok(json_response(403, &json!({ "error": "blocked by gateway WAF" }))) })
			});
			let responses_model = model(json!({
				"id": "gpt-test",
				"name": "GPT Test",
				"api": "openai-responses",
				"provider": "openai",
				"baseUrl": "https://api.openai.com/v1",
				"reasoning": false,
				"input": ["text"],
				"cost": { "input": 0, "output": 0, "cacheRead": 0, "cacheWrite": 0 },
				"contextWindow": 1000,
				"maxTokens": 100,
			}));
			let context = context(json!({
				"systemPrompt": "",
				"messages": [{ "role": "user", "content": [{ "type": "text", "text": "hi" }], "timestamp": 0 }],
				"tools": [],
			}));
			let stream = stream_open_ai_responses(
				&responses_model,
				&context,
				Some(responses_options(|options| options.api_key = Some("test".into()))),
			);
			while stream.next().await.is_some() {}
			let output = stream.result().await.expect("stream has a result");

			assert_eq!(output.stop_reason, StopReason::Error);
			let error_message = output.error_message.unwrap_or_default();
			assert!(error_message.contains("OpenAI API error (403)"), "{error_message}");
			assert!(error_message.contains("blocked by gateway WAF"), "{error_message}");
		});
	}
}
