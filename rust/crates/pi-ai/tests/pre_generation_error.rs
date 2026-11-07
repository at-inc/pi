//! Port of `packages/ai/test/pre-generation-error.test.ts`.
//!
//! The synchronous throw of the TS is the `Err` of each API's `stream_simple`.

use pi_ai::api::anthropic_messages::stream_simple as stream_anthropic;
use pi_ai::api::{
	azure_openai_responses, google_generative_ai, mistral_conversations, openai_codex_responses, openai_completions,
	openai_responses,
};
use pi_ai::types::{Model, SimpleStreamOptions};
use pi_ai::utils::transcript::normalize_context;
use pi_host::PiResult;
use serde_json::json;

fn model(api: &str) -> Model {
	serde_json::from_value(json!({
		"id": "test-model",
		"name": "Test",
		"api": api,
		"provider": "test-provider",
		"baseUrl": "https://example.invalid",
		"reasoning": false,
		"input": ["text"],
		"cost": { "input": 0, "output": 0, "cacheRead": 0, "cacheWrite": 0 },
		"contextWindow": 1_000,
		"maxTokens": 100,
	}))
	.unwrap()
}

/// `expectMissingAuthThrows(create)`.
fn expect_missing_auth_throws<T>(create: impl FnOnce() -> PiResult<T>) {
	let Err(error) = create() else {
		panic!("expected a throw");
	};
	assert!(
		error.message.contains("No API key for provider: test-provider"),
		"{}",
		error.message
	);
}

mod direct_api_authentication {
	use super::*;

	#[test]
	fn throws_synchronously_when_auth_is_missing() {
		pi_host::testing::run(|_| async {
			let context = normalize_context(&serde_json::from_value(json!({ "messages": [] })).unwrap());
			expect_missing_auth_throws(|| {
				stream_anthropic(
					&model("anthropic-messages"),
					&context,
					Some(SimpleStreamOptions::default()),
				)
			});
			expect_missing_auth_throws(|| {
				azure_openai_responses::stream_simple(
					&model("azure-openai-responses"),
					&context,
					Some(SimpleStreamOptions::default()),
				)
			});
			expect_missing_auth_throws(|| {
				google_generative_ai::stream_simple(
					&model("google-generative-ai"),
					&context,
					Some(SimpleStreamOptions::default()),
				)
			});
			expect_missing_auth_throws(|| {
				mistral_conversations::stream_simple(
					&model("mistral-conversations"),
					&context,
					Some(SimpleStreamOptions::default()),
				)
			});
			expect_missing_auth_throws(|| {
				openai_codex_responses::stream_simple(
					&model("openai-codex-responses"),
					&context,
					Some(SimpleStreamOptions::default()),
				)
			});
			expect_missing_auth_throws(|| {
				openai_completions::stream_simple(
					&model("openai-completions"),
					&context,
					Some(SimpleStreamOptions::default()),
				)
			});
			expect_missing_auth_throws(|| {
				openai_responses::stream_simple(
					&model("openai-responses"),
					&context,
					Some(SimpleStreamOptions::default()),
				)
			});
		});
	}
}
