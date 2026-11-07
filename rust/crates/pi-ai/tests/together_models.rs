//! Port of `packages/ai/test/together-models.test.ts`.
//!
//! `getModel` (compat.ts) is `get_builtin_model`. Not ported: the `generateModelsFixture` half of
//! `models Together reasoning controls from the Together API surface`; it runs `catalog/generate.ts`, a build
//! script that is not part of the Rust port. `process.env` is the `TestHost` environment, so the `afterEach`
//! restore is not needed.

use pi_ai::env_api_keys::{find_env_keys, get_env_api_key};
use pi_ai::providers::all::get_builtin_model;
use pi_ai::types::ModelInputModality;
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

mod together_models {
	use super::*;

	#[test]
	fn registers_the_default_kimi_k3_model_via_open_ai_compatible_chat_completions_api() {
		let model = get_builtin_model("together", "moonshotai/Kimi-K3").expect("model is defined");

		assert_eq!(model.api, "openai-completions");
		assert_eq!(model.provider, "together");
		assert_eq!(model.base_url, "https://api.together.ai/v1");
		assert!(model.reasoning);
		assert_eq!(
			serde_json::to_value(&model.thinking_level_map).unwrap(),
			json!({ "minimal": null, "low": null, "medium": null })
		);
		assert_eq!(model.input, [ModelInputModality::Text, ModelInputModality::Image]);
		assert_eq!(model.context_window, 1048576.0);
		assert_eq!(model.max_tokens, 131072.0);
		assert_match_object(
			&serde_json::to_value(&model.cost).unwrap(),
			&json!({ "input": 3, "output": 15, "cacheRead": 0.3, "cacheWrite": 0 }),
		);
		assert_eq!(
			serde_json::to_value(&model.compat).unwrap(),
			json!({
				"supportsStore": false,
				"supportsDeveloperRole": false,
				"supportsReasoningEffort": false,
				"maxTokensField": "max_tokens",
				"thinkingFormat": "together",
				"supportsStrictMode": false,
				"supportsLongCacheRetention": false,
			})
		);
	}

	#[test]
	fn models_together_reasoning_controls_from_the_together_api_surface() {
		let gpt_oss = get_builtin_model("together", "openai/gpt-oss-120b").unwrap();
		assert_eq!(
			serde_json::to_value(&gpt_oss.thinking_level_map).unwrap(),
			json!({
				"off": null,
				"minimal": null,
				"low": "low",
				"medium": "medium",
				"high": "high",
				"max": null,
				"xhigh": null,
			})
		);
		assert_match_object(
			&serde_json::to_value(&gpt_oss.compat).unwrap(),
			&json!({ "supportsReasoningEffort": true, "thinkingFormat": "openai" }),
		);
	}

	#[test]
	fn resolves_together_api_key_from_the_environment() {
		pi_host::testing::run(|host| async move {
			host.set_env("TOGETHER_API_KEY", "test-together-key");

			assert_eq!(
				find_env_keys("together", None),
				Some(vec!["TOGETHER_API_KEY".to_string()])
			);
			assert_eq!(get_env_api_key("together", None).as_deref(), Some("test-together-key"));
		});
	}
}
