//! Port of `packages/ai/test/pre-generation-error.test.ts`.
//!
//! `throws synchronously when auth is missing` is ported for `anthropic-messages`, `azure-openai-responses`,
//! `openai-codex-responses` and `openai-responses`: the synchronous throw is the `Err` of `stream_simple`. Not ported:
//! the same expectation for `google-generative-ai`, `mistral-conversations` and `openai-completions`, whose APIs
//! belong to other w3 shards.

use pi_ai::api::anthropic_messages::stream_simple as stream_anthropic;
use pi_ai::api::{azure_openai_responses, openai_codex_responses, openai_responses};
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
				openai_codex_responses::stream_simple(
					&model("openai-codex-responses"),
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
