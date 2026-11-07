//! Port of `packages/ai/test/model-catalog-types.test.ts`.
//!
//! `expectTypeOf` literal checks have no Rust equivalent (`api`, `id` and `provider` are `String`s), so they become
//! value checks on the same fields.

use pi_ai::providers::github_copilot_models::GITHUB_COPILOT_MODELS;
use pi_ai::providers::xai_models::XAI_MODELS;
use serde_json::{Value, json};

/// `expect(actual).toMatchObject(expected)` for JSON values; numbers compare as `f64`, like JS numbers.
fn assert_match_object(actual: &Value, expected: &Value) {
	match (actual, expected) {
		(Value::Object(actual), Value::Object(expected)) => {
			for (key, expected) in expected {
				assert_match_object(actual.get(key).unwrap_or(&Value::Null), expected);
			}
		}
		(Value::Array(actual), Value::Array(expected)) => {
			assert_eq!(actual.len(), expected.len());
			for (actual, expected) in actual.iter().zip(expected) {
				assert_match_object(actual, expected);
			}
		}
		(Value::Number(actual), Value::Number(expected)) => assert_eq!(actual.as_f64(), expected.as_f64()),
		_ => assert_eq!(actual, expected),
	}
}

#[test]
fn derives_model_api_id_and_provider_literals_from_grouped_model_data() {
	for id in ["grok-4.5", "grok-4.6", "grok-4.7", "grok-4.3"] {
		let model = &XAI_MODELS[id];
		assert_eq!(model.api, "openai-responses", "{id}");
		assert_eq!(model.id, id);
		assert_eq!(model.provider, "xai", "{id}");
	}
}

#[test]
fn routes_git_hub_copilot_grok_4_5_through_the_responses_api() {
	assert_eq!(GITHUB_COPILOT_MODELS["grok-4.5"].api, "openai-responses");
}

// Regression test for https://github.com/earendil-works/pi/issues/9209
#[test]
fn routes_all_git_hub_copilot_gpt_models_through_the_responses_api() {
	let gpt_models: Vec<_> = GITHUB_COPILOT_MODELS
		.values()
		.filter(|model| model.id.starts_with("gpt-"))
		.collect();
	assert!(!gpt_models.is_empty());
	assert!(gpt_models.iter().all(|model| model.api == "openai-responses"));
	assert_eq!(GITHUB_COPILOT_MODELS["gpt-6-astra"].api, "openai-responses");
	for model_id in ["gpt-6-sol", "gpt-6-luna"] {
		assert_match_object(
			&serde_json::to_value(&GITHUB_COPILOT_MODELS[model_id]).unwrap(),
			&json!({
				"api": "openai-responses",
				"contextWindow": 1000000,
				"maxTokens": 128000,
				"thinkingLevelMap": { "off": "none", "max": "max" },
			}),
		);
	}
}
