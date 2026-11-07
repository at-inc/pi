//! Port of `packages/ai/test/qwen-token-plan-models.test.ts`.
//!
//! `getModels` (compat.ts) is `get_builtin_models`. Not ported: `sends Qwen thinking fields for $provider/$modelId`,
//! `sends Qwen reasoning_effort for $provider/$modelId` and `sends qwen3.8 xhigh reasoning_effort for
//! $provider/$modelId`. They capture the request through `streamSimple` on the openai-completions API (wave-3,
//! `w3-openai-completions`) with the `openai` SDK mocked. `it.each` cases run in a loop.

use pi_ai::env_api_keys::find_env_keys;
use pi_ai::providers::all::get_builtin_models;
use pi_ai::types::{Model, ProviderEnv};
use serde_json::{Value, json};

/// `expect(actual).toMatchObject(expected)` for JSON values; numbers compare as `f64`, like JS numbers.
fn assert_match_object(actual: &Value, expected: &Value) {
	match (actual, expected) {
		(Value::Object(actual), Value::Object(expected)) => {
			for (key, expected) in expected {
				assert_match_object(actual.get(key).unwrap_or_else(|| panic!("missing key {key}")), expected);
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

const TEXT_MODELS: [&str; 16] = [
	"MiniMax-M2.5",
	"deepseek-v3.2",
	"deepseek-v4-flash",
	"deepseek-v4-pro",
	"glm-5",
	"glm-5.1",
	"glm-5.2",
	"kimi-k2.5",
	"kimi-k2.6",
	"kimi-k2.7-code",
	"qwen3.6-flash",
	"qwen3.6-plus",
	"qwen3.7-max",
	"qwen3.7-plus",
	"qwen3.8-flash",
	"qwen3.8-max",
];

const INDIVIDUAL_TEXT_MODELS: [&str; 9] = [
	"deepseek-v4-flash-0731",
	"deepseek-v4-pro",
	"deepseek-v4-pro-0813",
	"glm-5.2",
	"qwen3.6-flash",
	"qwen3.7-max",
	"qwen3.7-plus",
	"qwen3.8-flash",
	"qwen3.8-max",
];

const IMAGE_MODELS: [&str; 4] = [
	"qwen-image-2.0",
	"qwen-image-2.0-pro",
	"wan2.7-image",
	"wan2.7-image-pro",
];

const QWEN_REASONING_EFFORT_MODELS: [&str; 5] = ["deepseek-v4-flash", "deepseek-v4-pro", "glm-5", "glm-5.1", "glm-5.2"];
const QWEN38_MODELS: [&str; 2] = ["qwen3.8-flash", "qwen3.8-max"];

fn qwen_reasoning_effort_model_cases() -> Vec<(&'static str, &'static str)> {
	let mut cases: Vec<_> = ["qwen-token-plan", "qwen-token-plan-cn"]
		.into_iter()
		.flat_map(|provider| QWEN_REASONING_EFFORT_MODELS.map(|model_id| (provider, model_id)))
		.collect();
	cases.extend(
		[
			"deepseek-v4-flash-0731",
			"deepseek-v4-pro",
			"deepseek-v4-pro-0813",
			"glm-5.2",
		]
		.map(|model_id| ("qwen-token-plan-individual", model_id)),
	);
	cases
}

fn qwen38_model_cases() -> Vec<(&'static str, &'static str)> {
	["qwen-token-plan", "qwen-token-plan-cn", "qwen-token-plan-individual"]
		.into_iter()
		.flat_map(|provider| QWEN38_MODELS.map(|model_id| (provider, model_id)))
		.collect()
}

fn model_ids(provider: &str) -> Vec<String> {
	get_builtin_models(provider)
		.into_iter()
		.map(|model| model.id.clone())
		.collect()
}

fn find_model(provider: &str, model_id: &str) -> Model {
	get_builtin_models(provider)
		.into_iter()
		.find(|candidate| candidate.id == model_id)
		.unwrap_or_else(|| panic!("Missing model: {provider}/{model_id}"))
}

mod qwen_token_plan_models {
	use super::*;

	// #9021
	#[test]
	fn exposes_exactly_the_documented_individual_text_models() {
		let mut model_ids = model_ids("qwen-token-plan-individual");
		model_ids.sort();
		let mut expected = INDIVIDUAL_TEXT_MODELS.map(str::to_string).to_vec();
		expected.sort();

		assert_eq!(model_ids, expected);
	}

	#[test]
	fn reuses_the_international_token_plan_environment_variable() {
		let env = ProviderEnv::from([("QWEN_TOKEN_PLAN_API_KEY".to_string(), "test".to_string())]);
		pi_host::testing::run(|_| async move {
			assert_eq!(
				find_env_keys("qwen-token-plan-individual", Some(&env)),
				Some(vec!["QWEN_TOKEN_PLAN_API_KEY".to_string()])
			);
		});
	}

	#[test]
	fn exposes_all_text_models() {
		for provider in ["qwen-token-plan", "qwen-token-plan-cn"] {
			let model_ids = model_ids(provider);
			for expected in TEXT_MODELS {
				assert!(
					model_ids.iter().any(|id| id == expected),
					"{provider} should include {expected}"
				);
			}
		}
	}

	#[test]
	fn omits_image_models() {
		for provider in ["qwen-token-plan", "qwen-token-plan-cn"] {
			let model_ids = model_ids(provider);
			for excluded in IMAGE_MODELS {
				assert!(
					!model_ids.iter().any(|id| id == excluded),
					"{provider} should not include {excluded}"
				);
			}
		}
	}

	#[test]
	fn exposes_qwen_reasoning_effort_levels() {
		for (provider, model_id) in qwen_reasoning_effort_model_cases() {
			assert_match_object(
				&serde_json::to_value(&find_model(provider, model_id).thinking_level_map).unwrap(),
				&json!({ "minimal": null, "low": null, "medium": null, "high": "high", "xhigh": null, "max": "max" }),
			);
		}
	}

	#[test]
	fn exposes_qwen3_8_reasoning_effort_levels() {
		for (provider, model_id) in qwen38_model_cases() {
			assert_match_object(
				&serde_json::to_value(&find_model(provider, model_id).thinking_level_map).unwrap(),
				&json!({ "minimal": null, "low": "low", "medium": "medium", "high": null, "xhigh": "xhigh", "max": null }),
			);
		}
	}

	#[test]
	fn omits_retired_qwen3_8_max_preview() {
		for provider in ["qwen-token-plan", "qwen-token-plan-cn", "qwen-token-plan-individual"] {
			assert!(
				!model_ids(provider).iter().any(|id| id == "qwen3.8-max-preview"),
				"{provider}"
			);
		}
	}
}
