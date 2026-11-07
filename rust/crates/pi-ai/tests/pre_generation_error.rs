//! Port of `packages/ai/test/pre-generation-error.test.ts`, for the adapters w3-openai-responses ports.
//!
//! "throws synchronously when auth is missing" checks `api::azure_openai_responses`, `api::openai_codex_responses`
//! and `api::openai_responses` here: their `stream_simple` returns the error instead of a stream. The Anthropic,
//! Google, Mistral and OpenAI Completions adapters are still `PORT-STATUS: pending` on this branch; their shards add
//! them to this case.

use pi_ai::api::{azure_openai_responses, openai_codex_responses, openai_responses};
use pi_ai::types::{Context, Model, SimpleStreamOptions, TranscriptContext};
use pi_ai::utils::event_stream::AssistantMessageEventStream;
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
	.expect("model JSON")
}

fn expect_missing_auth_throws(create: impl FnOnce() -> PiResult<AssistantMessageEventStream>) {
	let error = create().err().expect("stream creation throws");
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
		let context: TranscriptContext = normalize_context(&Context::default());
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
	}
}
