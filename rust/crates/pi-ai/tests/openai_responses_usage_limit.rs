//! Port of `packages/ai/test/openai-responses-usage-limit.test.ts`.

mod openai_responses_support;

use openai_responses_support::{
	context, fetch_returning, gpt5_mini, json_response, response, responses_options, sse_body,
};
use pi_ai::api::openai_responses::stream;
use pi_ai::types::StopReason;
use pi_host::HttpResponse;
use pi_host::testing::run;
use serde_json::{Value, json};

fn usage_limit_error() -> Value {
	json!({ "code": "subscription_sharing_usage_limit_exceeded", "message": "Usage limit reached." })
}

async fn get_error_message(make: impl Fn() -> HttpResponse + 'static) -> Option<String> {
	let context = context(json!({
		"systemPrompt": "",
		"messages": [{ "role": "user", "content": [{ "type": "text", "text": "hi" }], "timestamp": 0 }],
		"tools": [],
	}));
	let options = responses_options(|options| {
		options.api_key = Some("test".into());
		options.fetch = Some(fetch_returning(make));
	});
	let result = stream(&gpt5_mini(), &context, Some(options))
		.result()
		.await
		.expect("stream has a result");
	assert_eq!(result.stop_reason, StopReason::Error);
	result.error_message
}

mod open_ai_responses_chat_gpt_usage_limit {
	use super::*;

	#[test]
	fn links_to_chatgpt_usage_when_the_request_is_rejected() {
		run(|_| async {
			let mut error = usage_limit_error();
			error["type"] = json!("rate_limit_error");
			let body = json!({ "error": error });
			let error_message = get_error_message(move || json_response(429, &body)).await.unwrap();

			assert!(
				error_message.contains("subscription_sharing_usage_limit_exceeded"),
				"{error_message}"
			);
			assert!(
				error_message.contains("Check your ChatGPT usage: https://chatgpt.com/settings/usage"),
				"{error_message}"
			);
		});
	}

	#[test]
	fn links_to_chatgpt_usage_when_the_stream_fails() {
		run(|_| async {
			let event = json!({
				"type": "response.failed",
				"sequence_number": 0,
				"response": { "id": "resp_failed", "status": "failed", "error": usage_limit_error() },
			});
			let body = sse_body(&[event]);
			let error_message =
				get_error_message(move || response(200, &[("content-type", "text/event-stream")], body.clone()))
					.await
					.unwrap();

			assert!(
				error_message.contains("subscription_sharing_usage_limit_exceeded: Usage limit reached."),
				"{error_message}"
			);
			assert!(
				error_message.contains("Check your ChatGPT usage: https://chatgpt.com/settings/usage"),
				"{error_message}"
			);
		});
	}
}
