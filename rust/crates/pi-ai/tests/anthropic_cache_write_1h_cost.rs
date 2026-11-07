//! Port of `packages/ai/test/anthropic-cache-write-1h-cost.test.ts`.
//!
//! Differences from the TS: the fake SDK client is `anthropic_support::FakeClient`, and `toBeCloseTo(x, 10)` is
//! `|actual - expected| < 5e-11`, vitest's bound for 10 digits.

mod anthropic_support;

use anthropic_support::{FakeClient, SseEvent, event, model, sse_response, user_context};
use pi_ai::api::anthropic_messages::{AnthropicOptions, stream};
use pi_ai::types::AssistantMessage;
use pi_host::HttpResponse;
use pi_host::testing::run;
use serde_json::{Map, Value, json};

/// `expect(actual).toBeCloseTo(expected, digits)`.
fn assert_close(actual: f64, expected: f64, digits: i32) {
	let bound = 10f64.powi(-digits) / 2.0;
	assert!(
		(actual - expected).abs() < bound,
		"expected {actual} to be close to {expected} ({digits} digits)"
	);
}

/// `eventsWithCacheCreation(cacheCreation)`.
fn events_with_cache_creation(cache_creation: Option<Value>) -> Vec<SseEvent> {
	let mut start_usage = Map::new();
	start_usage.insert("input_tokens".into(), json!(100));
	start_usage.insert("output_tokens".into(), json!(0));
	start_usage.insert("cache_read_input_tokens".into(), json!(0));
	start_usage.insert("cache_creation_input_tokens".into(), json!(1_000_000));
	if let Some(cache_creation) = cache_creation {
		start_usage.insert("cache_creation".into(), cache_creation);
	}
	vec![
		event(
			"message_start",
			json!({ "type": "message_start", "message": { "id": "msg_test", "usage": start_usage } }),
		),
		event(
			"content_block_start",
			json!({ "type": "content_block_start", "index": 0, "content_block": { "type": "text", "text": "" } }),
		),
		event(
			"content_block_delta",
			json!({ "type": "content_block_delta", "index": 0, "delta": { "type": "text_delta", "text": "Hi" } }),
		),
		event(
			"content_block_stop",
			json!({ "type": "content_block_stop", "index": 0 }),
		),
		event(
			"message_delta",
			json!({
				"type": "message_delta",
				"delta": { "stop_reason": "end_turn" },
				"usage": {
					"input_tokens": 100,
					"output_tokens": 5,
					"cache_read_input_tokens": 0,
					"cache_creation_input_tokens": 1_000_000,
				},
			}),
		),
		event("message_stop", json!({ "type": "message_stop" })),
	]
}

/// `streamAnthropic(model, context, { client: createFakeAnthropicClient(response) }).result()`.
async fn stream_with_fake_client(model: &pi_ai::types::Model, response: HttpResponse) -> AssistantMessage {
	// claude-opus-4-8: input 5, cacheWrite (5m) 6.25 per Mtok. 1h write = 2x input = 10.
	let context = user_context("hi");
	let options = AnthropicOptions {
		client: Some(FakeClient::new(response)),
		..AnthropicOptions::default()
	};
	stream(model, &context, Some(options)).result().await.unwrap()
}

mod anthropic_1h_cache_write_cost {
	use super::*;

	#[test]
	fn prices_the_1h_portion_at_2x_input_and_the_rest_at_the_5m_rate() {
		run(|_| async {
			let model = model("anthropic", "claude-opus-4-8");
			let response = sse_response(&events_with_cache_creation(Some(
				json!({ "ephemeral_5m_input_tokens": 600_000, "ephemeral_1h_input_tokens": 400_000 }),
			)));
			let result = stream_with_fake_client(&model, response).await;

			assert_eq!(result.usage.cache_write, 1_000_000.0);
			assert_eq!(result.usage.cache_write_1h, Some(400_000.0));
			// 600k * 6.25/Mtok + 400k * 10/Mtok = 3.75 + 4.0 = 7.75
			assert_close(result.usage.cost.cache_write, 7.75, 10);
		});
	}

	// Regression for #9210: Vercel AI Gateway sends cache usage in message_delta, not message_start.
	#[test]
	fn prices_1h_cache_writes_reported_only_in_message_delta() {
		run(|_| async {
			let model = model("vercel-ai-gateway", "anthropic/claude-haiku-4.5");
			let response = sse_response(&[
				event(
					"message_start",
					json!({
						"type": "message_start",
						"message": { "id": "msg_test", "usage": { "input_tokens": 0, "output_tokens": 0 } },
					}),
				),
				event(
					"message_delta",
					json!({
						"type": "message_delta",
						"delta": { "stop_reason": "end_turn" },
						"usage": {
							"input_tokens": 3,
							"output_tokens": 4,
							"cache_creation_input_tokens": 6535,
							"cache_creation": { "ephemeral_5m_input_tokens": 0, "ephemeral_1h_input_tokens": 6535 },
						},
					}),
				),
				event("message_stop", json!({ "type": "message_stop" })),
			]);
			let result = stream_with_fake_client(&model, response).await;

			assert_eq!(result.usage.cache_write, 6535.0);
			assert_eq!(result.usage.cache_write_1h, Some(6535.0));
			assert_close(
				result.usage.cost.cache_write,
				(6535.0 * model.cost.input * 2.0) / 1_000_000.0,
				10,
			);
		});
	}

	#[test]
	fn falls_back_to_the_5m_rate_when_no_breakdown_is_reported() {
		run(|_| async {
			let model = model("anthropic", "claude-opus-4-8");
			let response = sse_response(&events_with_cache_creation(None));
			let result = stream_with_fake_client(&model, response).await;

			assert_eq!(result.usage.cache_write, 1_000_000.0);
			assert_eq!(result.usage.cache_write_1h.unwrap_or(0.0), 0.0);
			// 1M * 6.25/Mtok = 6.25
			assert_close(result.usage.cost.cache_write, 6.25, 10);
		});
	}
}
