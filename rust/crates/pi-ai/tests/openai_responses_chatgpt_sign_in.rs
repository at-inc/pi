//! Port of `packages/ai/test/openai-responses-chatgpt-sign-in.test.ts`.

mod openai_responses_support;

use openai_responses_support::{
	capture_payload, context, fetch_returning, gpt5_mini, response, responses_options, with,
};
use pi_ai::api::openai_responses::stream;
use pi_ai::types::{CacheRetention, Model};
use pi_host::testing::run;
use serde_json::{Value, json};

async fn capture(api_key: &str, request_model: &Model) -> Value {
	let context = context(json!({
		"systemPrompt": "",
		"messages": [{ "role": "user", "content": [{ "type": "text", "text": "hi" }], "timestamp": 0 }],
		"tools": [],
	}));
	let (on_payload, payload) = capture_payload();
	let options = responses_options(|options| {
		options.api_key = Some(api_key.into());
		options.max_tokens = Some(1000.0);
		options.temperature = Some(0.5);
		options.cache_retention = Some(CacheRetention::Long);
		options.on_payload = Some(on_payload);
		options.fetch = Some(fetch_returning(|| response(500, &[], "")));
	});
	stream(request_model, &context, Some(options)).result().await;
	payload.borrow_mut().take().expect("Request payload was not captured")
}

mod open_ai_responses_with_sign_in_with_chat_gpt {
	use super::*;

	#[test]
	fn omits_request_fields_that_token_sharing_rejects() {
		run(|_| async {
			let payload = capture("chatgpt-access-token", &gpt5_mini()).await;

			assert!(payload.get("max_output_tokens").is_none());
			assert!(payload.get("temperature").is_none());
			assert!(payload.get("prompt_cache_retention").is_none());
		});
	}

	#[test]
	fn omits_prompt_cache_options_on_models_with_explicit_prompt_cache_mode() {
		run(|_| async {
			let explicit_cache_model = with(
				&gpt5_mini(),
				json!({ "compat": { "supportsExplicitPromptCacheMode": true } }),
			);

			let sign_in_payload = capture("chatgpt-access-token", &explicit_cache_model).await;
			let api_key_payload = capture("sk-proj-test", &explicit_cache_model).await;

			assert!(sign_in_payload.get("prompt_cache_options").is_none());
			assert_eq!(api_key_payload["prompt_cache_options"], json!({ "ttl": "30m" }));
		});
	}

	#[test]
	fn keeps_those_fields_for_name() {
		run(|_| async {
			let cases = [
				("OpenAI API keys", "sk-proj-test", gpt5_mini()),
				(
					"other OpenAI-compatible endpoints",
					"gateway-key",
					with(&gpt5_mini(), json!({ "baseUrl": "https://gateway.example.com/v1" })),
				),
			];
			for (name, api_key, request_model) in cases {
				let payload = capture(api_key, &request_model).await;

				assert_eq!(payload["max_output_tokens"].as_f64(), Some(1000.0), "{name}");
				assert_eq!(payload["temperature"], json!(0.5), "{name}");
				assert_eq!(payload["prompt_cache_retention"], json!("24h"), "{name}");
			}
		});
	}
}
