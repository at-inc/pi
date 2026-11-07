//! Port of `packages/ai/test/max-thinking.test.ts`.
//!
//! `getModel`, `getSupportedThinkingLevels` and `clampThinkingLevel` (compat.ts) are `get_builtin_model` and the
//! `models.rs` functions. Not ported: `sends max to the Codex Responses API for %s`, which captures the request
//! through the openai-codex-responses API (wave-3, `w3-openai-responses`).

use pi_ai::models::{clamp_thinking_level, get_supported_thinking_levels};
use pi_ai::providers::all::get_builtin_model;
use pi_ai::providers::model_schema::{ModelThinkingLevel, ThinkingLevelMap};
use pi_ai::types::{BaseModel, Model, ModelCost, ModelInputModality};
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

use ModelThinkingLevel::{High, Low, Max, Medium, Minimal, Off, Xhigh};

fn reasoning_model(id: &str, name: &str, thinking_level_map: Option<ThinkingLevelMap>) -> Model {
	Model {
		base: BaseModel {
			id: id.into(),
			name: name.into(),
			api: "openai-completions".into(),
			provider: "test".into(),
			base_url: "https://example.com/v1".into(),
			input: vec![ModelInputModality::Text],
			cost: ModelCost::default(),
			..BaseModel::default()
		},
		reasoning: true,
		thinking_level_map,
		context_window: 128000.0,
		max_tokens: 4096.0,
		..Model::default()
	}
}

mod max_thinking_level {
	use super::*;

	#[test]
	fn is_opt_in_for_ordinary_reasoning_models() {
		let model = reasoning_model("ordinary-reasoning", "Ordinary Reasoning", None);

		assert_eq!(get_supported_thinking_levels(&model), [Off, Minimal, Low, Medium, High]);
		assert_eq!(clamp_thinking_level(&model, Max), High);
	}

	#[test]
	fn exposes_xhigh_and_max_for_openai_codex() {
		for model_id in [
			"gpt-5.6-luna",
			"gpt-5.6-sol",
			"gpt-5.6-terra",
			"gpt-6-luna",
			"gpt-6-sol",
		] {
			let model = get_builtin_model("openai-codex", model_id).expect("model is defined");
			assert_match_object(
				&serde_json::to_value(&model.thinking_level_map).unwrap(),
				&json!({ "xhigh": "xhigh", "max": "max" }),
			);
			assert_eq!(
				get_supported_thinking_levels(&model),
				[Off, Minimal, Low, Medium, High, Xhigh, Max],
				"{model_id}"
			);
		}
	}

	#[test]
	fn supports_a_hole_between_high_and_max() {
		let model = reasoning_model(
			"high-and-max",
			"High and Max",
			Some(serde_json::from_value(json!({ "xhigh": null, "max": "max" })).unwrap()),
		);

		assert_eq!(
			get_supported_thinking_levels(&model),
			[Off, Minimal, Low, Medium, High, Max]
		);
		assert_eq!(clamp_thinking_level(&model, Xhigh), Max);
	}
}
