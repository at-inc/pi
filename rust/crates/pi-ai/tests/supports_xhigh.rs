//! Port of `packages/ai/test/supports-xhigh.test.ts`.
//!
//! `getModel`, `getModels` and `getSupportedThinkingLevels` (compat.ts) are `get_builtin_model`,
//! `get_builtin_models` and the `models.rs` function. Not ported, because `generateModelsFixture` runs
//! `catalog/generate.ts`, a build script outside the Rust port: `includes low/high/max plus off for DeepSeek V4
//! Flash on opencode-go` and the fixture half of `does not advertise retired Codex model %s`. `it.each` cases run
//! in a loop.

use pi_ai::models::get_supported_thinking_levels;
use pi_ai::providers::all::{get_builtin_model, get_builtin_models};
use pi_ai::providers::model_schema::ModelThinkingLevel;
use pi_ai::types::Model;
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

fn model(provider: &str, id: &str) -> Model {
	get_builtin_model(provider, id).unwrap_or_else(|| panic!("{provider}/{id} is missing"))
}

fn levels(provider: &str, id: &str) -> Vec<ModelThinkingLevel> {
	get_supported_thinking_levels(&model(provider, id))
}

/// `model.thinkingLevelMap?.[level]`, with `undefined` and `null` both as `Value::Null`.
fn thinking_level(model: &Model, level: &str) -> Value {
	serde_json::to_value(&model.thinking_level_map).unwrap()[level].clone()
}

mod get_supported_thinking_levels {
	use super::*;

	#[test]
	fn includes_max_but_not_xhigh_for_anthropic_opus_4_6_on_anthropic_messages_api() {
		let levels = levels("anthropic", "claude-opus-4-6");
		assert!(levels.contains(&Max));
		assert!(!levels.contains(&Xhigh));
	}

	#[test]
	fn includes_xhigh_and_max_for_anthropic_opus_4_8_on_anthropic_messages_api() {
		let levels = levels("anthropic", "claude-opus-4-8");
		assert!(levels.contains(&Xhigh));
		assert!(levels.contains(&Max));
	}

	#[test]
	fn includes_xhigh_and_max_for_anthropic_opus_5_on_anthropic_messages_api() {
		let levels = levels("anthropic", "claude-opus-5");
		assert!(levels.contains(&Xhigh));
		assert!(levels.contains(&Max));
	}

	#[test]
	fn includes_claude_opus_5_5_with_its_always_on_effort_levels_and_official_pricing() {
		let model = model("anthropic", "claude-opus-5-5");
		assert_match_object(
			&serde_json::to_value(&model).unwrap(),
			&json!({
				"cost": { "input": 4, "output": 20, "cacheRead": 0.2, "cacheWrite": 5 },
				"contextWindow": 1_000_000,
				"maxTokens": 128_000,
				"compat": {
					"forceAdaptiveThinking": true,
					"supportsMidConvoEffort": true,
					"supportsMidConvoSystemMessages": true,
					"supportsMidConvoToolChanges": true,
				},
			}),
		);
		assert_eq!(get_supported_thinking_levels(&model), [Low, Medium, High, Xhigh, Max]);
	}

	#[test]
	fn includes_claude_sonnet_5_5_with_managed_effort_levels_and_official_pricing() {
		let model = model("anthropic", "claude-sonnet-5-5");
		assert_match_object(
			&serde_json::to_value(&model).unwrap(),
			&json!({
				"cost": { "input": 2, "output": 10, "cacheRead": 0.1, "cacheWrite": 2.5 },
				"contextWindow": 1_000_000,
				"maxTokens": 128_000,
				"compat": {
					"forceAdaptiveThinking": true,
					"supportsMidConvoEffort": true,
					"supportsMidConvoSystemMessages": true,
					"supportsMidConvoToolChanges": true,
					"supportsTemperature": false,
				},
			}),
		);
		assert_eq!(get_supported_thinking_levels(&model), [Low, Medium, High, Xhigh, Max]);
	}

	#[test]
	fn includes_max_but_not_xhigh_for_anthropic_sonnet_4_6_on_anthropic_messages_api() {
		let levels = levels("anthropic", "claude-sonnet-4-6");
		assert!(levels.contains(&Max));
		assert!(!levels.contains(&Xhigh));
	}

	#[test]
	fn includes_xhigh_and_max_for_anthropic_sonnet_5_on_anthropic_messages_api() {
		let levels = levels("anthropic", "claude-sonnet-5");
		assert!(levels.contains(&Xhigh));
		assert!(levels.contains(&Max));
	}

	#[test]
	fn includes_xhigh_and_max_but_not_off_for_anthropic_claude_fable_5_on_anthropic_messages_api() {
		let levels = levels("anthropic", "claude-fable-5");
		assert!(levels.contains(&Xhigh));
		assert!(levels.contains(&Max));
		assert!(!levels.contains(&Off));
	}

	#[test]
	fn does_not_include_xhigh_or_max_for_claude_sonnet_4_5() {
		let levels = levels("anthropic", "claude-sonnet-4-5");
		assert!(!levels.contains(&Xhigh));
		assert!(!levels.contains(&Max));
	}

	#[test]
	fn includes_xhigh_for_openai_codex_models() {
		for model_id in [
			"gpt-5.6-sol",
			"gpt-5.6-terra",
			"gpt-5.6-luna",
			"gpt-6-astra",
			"gpt-6-sol",
			"gpt-6-luna",
			"gpt-6.1-sol",
			"gpt-6.1-terra",
		] {
			assert!(levels("openai-codex", model_id).contains(&Xhigh), "{model_id}");
		}
	}

	#[test]
	fn includes_xhigh_and_max_for_openai_models() {
		for model_id in [
			"gpt-5.6-sol",
			"gpt-5.6-terra",
			"gpt-5.6-luna",
			"gpt-6-sol",
			"gpt-6-luna",
			"gpt-6.1-terra",
		] {
			assert_eq!(
				levels("openai", model_id),
				[Off, Low, Medium, High, Xhigh, Max],
				"{model_id}"
			);
		}
	}

	#[test]
	fn includes_xhigh_and_max_but_not_off_or_minimal_for_openai_gpt_6_astra() {
		let model = model("openai", "gpt-6-astra");
		assert_match_object(
			&serde_json::to_value(&model).unwrap(),
			&json!({
				"id": "gpt-6-astra",
				"name": "GPT-6 Astra",
				"contextWindow": 272000,
				"maxTokens": 128000,
				"cost": {
					"input": 10,
					"output": 50,
					"cacheRead": 1,
					"cacheWrite": 12.5,
					"tiers": [{ "inputTokensAbove": 272000, "input": 20, "output": 75, "cacheRead": 2, "cacheWrite": 25 }],
				},
				"thinkingLevelMap": { "off": null, "minimal": null, "xhigh": "xhigh", "max": "max" },
				"compat": {
					"supportsToolSearch": true,
					"supportsAdditionalTools": true,
					"supportsExplicitPromptCacheMode": true,
				},
			}),
		);
		assert_eq!(get_supported_thinking_levels(&model), [Low, Medium, High, Xhigh, Max]);
		let azure_model = super::model("azure", "gpt-6-astra");
		assert_eq!(azure_model.context_window, 1050000.0);
		assert_eq!(
			get_supported_thinking_levels(&azure_model),
			[Low, Medium, High, Xhigh, Max]
		);
	}

	#[test]
	fn includes_xhigh_and_max_but_not_off_or_minimal_for_openai_codex_gpt_6_astra() {
		let model = model("openai-codex", "gpt-6-astra");
		assert_match_object(
			&serde_json::to_value(&model).unwrap(),
			&json!({
				"id": "gpt-6-astra",
				"name": "GPT-6 Astra",
				"contextWindow": 272000,
				"maxTokens": 128000,
				"thinkingLevelMap": { "off": null, "minimal": null, "xhigh": "xhigh", "max": "max" },
				"compat": { "supportsToolSearch": true, "supportsAdditionalTools": true },
			}),
		);
		assert_eq!(get_supported_thinking_levels(&model), [Low, Medium, High, Xhigh, Max]);
	}

	// OpenAI and Codex reject reasoning.effort "none" for GPT-6.1 Sol.
	#[test]
	fn does_not_support_off_for_gpt_6_1_sol() {
		let expected: [(&str, &[ModelThinkingLevel]); 3] = [
			("openai", &[Low, Medium, High, Xhigh, Max]),
			("azure", &[Low, Medium, High, Xhigh, Max]),
			("openai-codex", &[Minimal, Low, Medium, High, Xhigh, Max]),
		];
		for (provider, levels) in expected {
			let model = model(provider, "gpt-6.1-sol");
			assert_eq!(get_supported_thinking_levels(&model), levels, "{provider}");
			assert_eq!(thinking_level(&model, "off"), Value::Null, "{provider}");
		}
	}

	// GPT-6.1 Terra accepts reasoning.effort "none" but, like GPT-6.1 Sol, rejects "minimal".
	#[test]
	fn supports_off_but_not_minimal_for_gpt_6_1_terra() {
		for provider in ["openai", "azure", "openai-codex"] {
			let model = model(provider, "gpt-6.1-terra");
			assert_eq!(thinking_level(&model, "off"), json!("none"), "{provider}");
			let minimal = if provider == "openai-codex" {
				json!("low")
			} else {
				Value::Null
			};
			assert_eq!(thinking_level(&model, "minimal"), minimal, "{provider}");
		}
	}

	#[test]
	fn includes_official_metadata_for_openai_and_codex() {
		// Codex serves GPT-6.1 Sol with a 500K default context window (#10840)
		for (model_id, cost, codex_context_window) in [
			("gpt-6-sol", [2.0, 10.0, 0.2, 2.5], 272000),
			("gpt-6-luna", [0.1, 0.5, 0.01, 0.125], 272000),
			("gpt-6.1-sol", [2.0, 10.0, 0.1, 2.5], 500000),
			("gpt-6.1-terra", [1.0, 5.0, 0.05, 1.25], 500000),
		] {
			let [input, output, cache_read, cache_write] = cost;
			for provider in ["openai", "openai-codex"] {
				assert_match_object(
					&serde_json::to_value(model(provider, model_id)).unwrap(),
					&json!({
						"input": ["text", "image"],
						"cost": {
							"input": input,
							"output": output,
							"cacheRead": cache_read,
							"cacheWrite": cache_write,
							"tiers": [{
								"inputTokensAbove": 272000,
								"input": input * 2.0,
								"output": output * 1.5,
								"cacheRead": cache_read * 2.0,
								"cacheWrite": cache_write * 2.0,
							}],
						},
						"contextWindow": if provider == "openai-codex" { codex_context_window } else { 272000 },
						"maxTokens": 128000,
						"compat": {
							"supportsAdditionalTools": true,
							"supportsMidConvoSystemMessages": true,
							"supportsOpenAIGrammarTools": true,
							"supportsToolSearch": true,
						},
					}),
				);
			}
		}
	}

	#[test]
	fn uses_the_model_specific_codex_context_window() {
		for (model_id, context_window) in [
			("gpt-6.1-sol", 500000.0),
			("gpt-6-astra", 272000.0),
			("gpt-6-sol", 272000.0),
			("gpt-6-luna", 272000.0),
			("gpt-5.6-sol", 272000.0),
			("gpt-5.6-terra", 272000.0),
			("gpt-5.6-luna", 272000.0),
		] {
			let model = model("openai-codex", model_id);
			assert_eq!(model.context_window, context_window, "{model_id}");
			assert_eq!(model.max_tokens, 128000.0, "{model_id}");
		}
	}

	#[test]
	fn does_not_advertise_retired_codex_model() {
		for model_id in ["gpt-5.5", "gpt-5.3-codex-spark"] {
			assert!(
				!get_builtin_models("openai-codex")
					.iter()
					.any(|model| model.id == model_id),
				"{model_id}"
			);
		}
	}

	#[test]
	fn includes_only_medium_high_xhigh_for_openai_gpt_5_5_pro() {
		assert_eq!(levels("openai", "gpt-5.5-pro"), [Medium, High, Xhigh]);
	}

	#[test]
	fn includes_only_medium_high_xhigh_for_openrouter_gpt_5_5_pro() {
		assert_eq!(levels("openrouter", "openai/gpt-5.5-pro"), [Medium, High, Xhigh]);
	}

	#[test]
	fn includes_low_high_max_plus_off_for_deepseek_v4_1_flash_on_the_deepseek_provider() {
		assert_eq!(levels("deepseek", "deepseek-flash"), [Off, Low, High, Max]);
	}

	#[test]
	fn preserves_low_high_max_metadata_for_deepseek_v4_1_flash_on_openrouter() {
		assert_eq!(
			levels("openrouter", "deepseek/deepseek-v4.1-flash"),
			[Off, Low, High, Max]
		);
	}

	#[test]
	fn preserves_low_high_max_metadata_for_deepseek_v4_1_flash_on_opencode_go() {
		assert_eq!(levels("opencode-go", "deepseek-v4.1-flash"), [Low, High, Max]);
	}

	#[test]
	fn excludes_thinking_off_for_moonshot_kimi_k2_7_code_models() {
		for provider in ["moonshotai", "moonshotai-cn"] {
			assert_eq!(
				levels(provider, "kimi-k2.7-code"),
				[Minimal, Low, Medium, High],
				"{provider}"
			);
		}
	}

	#[test]
	fn uses_the_verified_effort_options_for_kimi_k3() {
		for provider in ["moonshotai", "moonshotai-cn"] {
			assert_eq!(levels(provider, "kimi-k3"), [Low, High, Max], "{provider}");
		}
	}

	#[test]
	fn includes_only_low_high_max_for_kimi_coding_k3() {
		assert_eq!(levels("kimi-coding", "k3"), [Low, High, Max]);
	}

	#[test]
	fn excludes_off_for_always_thinking_kimi_coding_highspeed() {
		assert!(!levels("kimi-coding", "kimi-for-coding-highspeed").contains(&Off));
	}

	#[test]
	fn includes_only_high_for_opencode_grok_build() {
		assert_eq!(levels("opencode", "grok-build-0.1"), [High]);
	}

	#[test]
	fn includes_only_high_xhigh_plus_off_for_deepseek_v4_flash_on_openrouter() {
		assert_eq!(levels("openrouter", "deepseek/deepseek-v4-flash"), [Off, High, Xhigh]);
	}

	#[test]
	fn includes_max_but_not_xhigh_for_openrouter_opus_4_6() {
		let levels = levels("openrouter", "anthropic/claude-opus-4.6");
		assert!(levels.contains(&Max));
		assert!(!levels.contains(&Xhigh));
	}

	#[test]
	fn includes_xhigh_and_max_for_bedrock_claude_opus_5() {
		let levels = levels("amazon-bedrock", "global.anthropic.claude-opus-5");
		assert!(levels.contains(&Xhigh));
		assert!(levels.contains(&Max));
	}

	#[test]
	fn includes_xhigh_but_not_off_or_max_for_xai_grok_4_6() {
		assert_eq!(levels("xai", "grok-4.6"), [Low, Medium, High, Xhigh]);
	}

	#[test]
	fn includes_xhigh_and_max_but_not_off_for_bedrock_claude_fable_5() {
		let levels = levels("amazon-bedrock", "global.anthropic.claude-fable-5");
		assert!(levels.contains(&Xhigh));
		assert!(levels.contains(&Max));
		assert!(!levels.contains(&Off));
	}
}
