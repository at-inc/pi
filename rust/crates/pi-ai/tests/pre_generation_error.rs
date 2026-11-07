//! Port of `packages/ai/test/pre-generation-error.test.ts`.
//!
//! `throws synchronously when auth is missing` is ported for `anthropic-messages` only: its synchronous throw is the
//! `Err` of `stream_simple`. Not ported: the same expectation for `azure-openai-responses`, `google-generative-ai`,
//! `mistral-conversations`, `openai-codex-responses`, `openai-completions` and `openai-responses`, whose APIs
//! belong to other w3 shards.

use pi_ai::api::anthropic_messages::stream_simple as stream_anthropic;
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
		});
	}
}
