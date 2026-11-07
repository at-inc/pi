//! Port of `packages/ai/test/zai-coding-plan-models.test.ts`.

use pi_ai::providers::all::get_builtin_model;
use pi_ai::types::ModelCost;
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

fn cost(input: f64, output: f64, cache_read: f64) -> ModelCost {
	ModelCost {
		input,
		output,
		cache_read,
		cache_write: 0.0,
		..ModelCost::default()
	}
}

fn builtin_cost(provider: &str, id: &str) -> ModelCost {
	get_builtin_model(provider, id)
		.unwrap_or_else(|| panic!("{provider}/{id} is missing"))
		.cost
		.clone()
}

#[test]
fn exposes_glm_4_6v_on_the_china_coding_plan_catalog() {
	let model = get_builtin_model("zai-coding-cn", "glm-4.6v").unwrap();

	assert_match_object(
		&serde_json::to_value(&model).unwrap(),
		&json!({
			"id": "glm-4.6v",
			"provider": "zai-coding-cn",
			"api": "openai-completions",
			"baseUrl": "https://open.bigmodel.cn/api/coding/paas/v4",
			"reasoning": true,
			"input": ["text", "image"],
			"cost": { "input": 0.3, "output": 0.9, "cacheRead": 0.05, "cacheWrite": 0 },
			"contextWindow": 128000,
			"maxTokens": 32768,
			"compat": {
				"maxTokensField": "max_tokens",
				"thinkingFormat": "zai",
				"zaiToolStream": true,
			},
		}),
	);
}

#[test]
fn uses_api_equivalent_reference_costs_for_coding_plan_models() {
	assert_eq!(builtin_cost("zai", "glm-5.2"), cost(1.4, 4.4, 0.26));
	for provider in ["zai", "zai-coding-cn"] {
		assert_eq!(builtin_cost(provider, "glm-5.3"), cost(1.4, 4.4, 0.26));
		assert_eq!(builtin_cost(provider, "glm-5.3-flash"), cost(0.15, 0.5, 0.03));
	}
}

#[test]
fn keeps_zero_costs_for_coding_plan_models_without_a_matching_api_price() {
	let zero_cost = cost(0.0, 0.0, 0.0);

	assert_eq!(builtin_cost("zai", "glm-5.2-highspeed"), zero_cost);

	for provider in ["zai", "zai-coding-cn"] {
		assert_eq!(builtin_cost(provider, "glm-5.3-highspeed"), zero_cost);
	}
}
