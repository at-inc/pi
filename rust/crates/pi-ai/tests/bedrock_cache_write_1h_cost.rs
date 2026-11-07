//! Port of `packages/ai/test/bedrock-cache-write-1h-cost.test.ts`.
//!
//! Differences from the TS: the TS mocks `@aws-sdk/client-bedrock-runtime` and yields decoded stream items. Here
//! `bedrock_support::serve` answers with the same items as event stream frames, and `AWS_BEARER_TOKEN_BEDROCK` is set
//! so the client can authenticate. `toBeCloseTo(x, 10)` is `|actual - expected| < 5e-11`, vitest's bound for 10 digits.

mod bedrock_support;

use bedrock_support::{builtin_model, event, serve, stream_response};
use pi_ai::api::bedrock_converse_stream::{BedrockOptions, stream};
use pi_ai::types::{CacheRetention, StreamOptions};
use pi_host::testing::run;
use serde_json::json;

/// `expect(actual).toBeCloseTo(expected, digits)`.
fn assert_close(actual: f64, expected: f64, digits: i32) {
	let bound = 10f64.powi(-digits) / 2.0;
	assert!(
		(actual - expected).abs() < bound,
		"expected {actual} to be close to {expected} ({digits} digits)"
	);
}

mod bedrock_1h_cache_write_cost {
	use super::*;

	#[test]
	fn prices_the_1h_cache_details_at_2x_while_preserving_the_total_cache_write() {
		// Regression test for https://github.com/earendil-works/pi/issues/9457
		run(|host| async move {
			host.set_env("AWS_BEARER_TOKEN_BEDROCK", "tok");
			serve(
				&host,
				vec![stream_response(
					&[
						event("messageStart", json!({ "role": "assistant" })),
						event(
							"metadata",
							json!({
								"usage": {
									"inputTokens": 100,
									"outputTokens": 5,
									"totalTokens": 1_000_105,
									"cacheWriteInputTokens": 1_000_000,
									"cacheDetails": [
										{ "ttl": "1h", "inputTokens": 150_000 },
										{ "ttl": "5m", "inputTokens": 600_000 },
										{ "ttl": "1h", "inputTokens": 250_000 },
									],
								},
							}),
						),
						event("messageStop", json!({ "stopReason": "end_turn" })),
					],
					&[],
				)],
			);
			let model = builtin_model("us.anthropic.claude-opus-4-8");
			let context =
				bedrock_support::context(json!({ "messages": [{ "role": "user", "content": "hi", "timestamp": 1 }] }));
			let options = BedrockOptions {
				base: StreamOptions {
					cache_retention: Some(CacheRetention::None),
					..StreamOptions::default()
				},
				..BedrockOptions::default()
			};
			let result = stream(&model, &context, Some(options)).result().await.unwrap();

			assert_eq!(result.usage.cache_write, 1_000_000.0);
			assert_eq!(result.usage.cache_write_1h, Some(400_000.0));
			let expected_cache_write_cost =
				(600_000.0 * model.cost.cache_write + 400_000.0 * model.cost.input * 2.0) / 1_000_000.0;
			assert_close(result.usage.cost.cache_write, expected_cache_write_cost, 10);
		});
	}
}
