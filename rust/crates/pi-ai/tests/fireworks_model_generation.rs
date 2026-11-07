//! Port of the Anthropic Messages half of `packages/ai/test/fireworks-model-generation.test.ts`.
//!
//! The suite runs `catalog/generate.ts` (catalog build tooling, not part of the Rust port) against a mocked
//! models.dev catalog and checks the generated JSON; those checks are not ported. "automatically sends native effort
//! for newly cataloged Messages models" also streams the generated model; that half is ported against the model the
//! generator produces for `new-reasoner` (toggle + `effort: [low, max]`): `getEffortThinkingLevelMap` gives
//! `{ off: null, minimal: null, low: "low", medium: null, high: null, xhigh: null, max: "max" }`, the toggle merges
//! `off: "none"`, and the Fireworks provider adds its Anthropic compat plus `forceAdaptiveThinking: true`.

use std::cell::RefCell;
use std::rc::Rc;

use pi_ai::api::anthropic_messages::stream_simple;
use pi_ai::models::get_supported_thinking_levels;
use pi_ai::providers::model_schema::ModelThinkingLevel::{Low, Max, Off};
use pi_ai::types::{Context, Model, ProviderRequestOptions, SimpleStreamOptions, StreamOptions, ThinkingLevel};
use pi_ai::utils::transcript::normalize_context;
use pi_host::PiError;
use pi_host::testing::run;
use serde_json::{Value, json};

fn new_reasoner() -> Model {
	serde_json::from_value(json!({
		"id": "accounts/fireworks/models/new-reasoner",
		"name": "new-reasoner",
		"api": "anthropic-messages",
		"provider": "fireworks",
		"baseUrl": "https://api.fireworks.ai/inference",
		"reasoning": true,
		"thinkingLevelMap": {
			"off": "none", "minimal": null, "low": "low", "medium": null, "high": null, "xhigh": null, "max": "max",
		},
		"input": ["text"],
		"cost": { "input": 0, "output": 0, "cacheRead": 0, "cacheWrite": 0 },
		"contextWindow": 128000,
		"maxTokens": 16384,
		"compat": {
			"allowEmptySignature": true,
			"sendSessionAffinityHeaders": true,
			"supportsEagerToolInputStreaming": false,
			"supportsCacheControlOnTools": false,
			"supportsLongCacheRetention": false,
			"forceAdaptiveThinking": true,
		},
	}))
	.unwrap()
}

mod fireworks_model_generation {
	use super::*;

	// Regression for #9323: new effort-capable models must not require an allowlist update.
	#[test]
	fn automatically_sends_native_effort_for_newly_cataloged_messages_models() {
		let model = new_reasoner();
		assert_eq!(model.compat.as_ref().unwrap()["forceAdaptiveThinking"], json!(true));
		assert_eq!(get_supported_thinking_levels(&model), [Off, Low, Max]);
		run(|_| async {
			let captured: Rc<RefCell<Option<Value>>> = Rc::default();
			let sink = captured.clone();
			let base = StreamOptions {
				base: ProviderRequestOptions {
					api_key: Some("test-fireworks-key".into()),
					on_payload: Some(Rc::new(move |payload, _| {
						*sink.borrow_mut() = Some(payload);
						Box::pin(async { Err(PiError::new("payload captured")) })
					})),
					..ProviderRequestOptions::default()
				},
				..StreamOptions::default()
			};
			let options = SimpleStreamOptions {
				base,
				reasoning: Some(ThinkingLevel::Max),
				..SimpleStreamOptions::default()
			};
			let context: Context =
				serde_json::from_value(json!({ "messages": [{ "role": "user", "content": "test", "timestamp": 0 }] }))
					.unwrap();
			stream_simple(&model, &normalize_context(&context), Some(options))
				.unwrap()
				.result()
				.await;
			let payload = captured.borrow_mut().take().expect("payload captured");
			assert_eq!(
				payload["thinking"],
				json!({ "type": "adaptive", "display": "summarized" })
			);
			assert_eq!(payload["output_config"], json!({ "effort": "max" }));
		});
	}
}
