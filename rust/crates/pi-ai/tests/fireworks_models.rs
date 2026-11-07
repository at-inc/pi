//! Port of `packages/ai/test/fireworks-models.test.ts`.
//!
//! `getModel` (compat.ts) is `get_builtin_model`. `process.env` is the `TestHost` environment, so the `afterEach`
//! restore is not needed. Not ported, because they capture requests through wave-3 APIs: `omits unsupported long
//! cache retention for %s`, the payload half of `routes Kimi K3 through the OpenAI-compatible API with native effort
//! controls` (openai-completions, `w3-openai-completions`), the payload half of `sends native Messages effort levels
//! for %s`, the payload half of `keeps toggle-only Messages models without a verified fallback on budget-based
//! thinking`, and the whole `Anthropic-compatible session affinity and tool compat` describe (anthropic-messages,
//! `w3-anthropic`, against a local `node:http` server). They move with those APIs.

use pi_ai::env_api_keys::{find_env_keys, get_env_api_key};
use pi_ai::models::get_supported_thinking_levels;
use pi_ai::providers::all::get_builtin_model;
use pi_ai::providers::model_schema::ModelThinkingLevel;
use pi_ai::types::{Model, ModelInputModality};
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

/// `expect(actual).toEqual(expected)` for JSON objects: the same keys, values as in `assert_match_object`.
fn assert_json_eq(actual: &Value, expected: &Value) {
	assert_eq!(
		actual
			.as_object()
			.map(|object| object.keys().collect::<std::collections::BTreeSet<_>>()),
		expected
			.as_object()
			.map(|object| object.keys().collect::<std::collections::BTreeSet<_>>())
	);
	assert_match_object(actual, expected);
}

use ModelThinkingLevel::{High, Low, Max, Medium, Off, Xhigh};

fn fireworks(model_id: &str) -> Model {
	get_builtin_model("fireworks", model_id).unwrap_or_else(|| panic!("fireworks/{model_id} is missing"))
}

fn to_json(value: impl serde::Serialize) -> Value {
	serde_json::to_value(value).unwrap()
}

mod fireworks_models {
	use super::*;

	#[test]
	fn registers_non_glm_non_kimi_k3_models_via_anthropic_compatible_messages_api() {
		let model = fireworks("accounts/fireworks/models/deepseek-v4p1-flash");

		assert_eq!(model.api, "anthropic-messages");
		assert_eq!(model.provider, "fireworks");
		assert_eq!(model.base_url, "https://api.fireworks.ai/inference");
		assert!(model.reasoning);
		assert_eq!(model.input, [ModelInputModality::Text, ModelInputModality::Image]);
	}

	#[test]
	fn aligns_glm_5_3_fast_with_glm_5_3s_open_ai_compatible_config() {
		let base = fireworks("accounts/fireworks/models/glm-5p3");
		let fast = fireworks("accounts/fireworks/routers/glm-5p3-fast");

		assert_eq!(fast.api, base.api);
		assert_eq!(fast.base_url, base.base_url);
		assert_eq!(fast.compat, base.compat);
		assert_eq!(fast.thinking_level_map, base.thinking_level_map);
	}

	#[test]
	fn routes_kimi_k3_through_the_open_ai_compatible_api_with_native_effort_controls() {
		let base = fireworks("accounts/fireworks/models/kimi-k3");
		let fast = fireworks("accounts/fireworks/routers/kimi-k3-fast");
		let compat = json!({
			"supportsStore": false,
			"supportsDeveloperRole": false,
			"supportsStrictMode": true,
			"requiresReasoningContentOnAssistantMessages": true,
			"thinkingFormat": "openai",
			"supportsMidConvoSystemMessages": true,
			"supportsMidConvoToolAdditions": true,
			"sendSessionAffinityHeaders": true,
			"supportsLongCacheRetention": false,
		});
		let thinking_level_map = json!({
			"off": null,
			"minimal": null,
			"low": "low",
			"medium": null,
			"high": "high",
			"xhigh": null,
			"max": "max",
		});

		assert_eq!(base.api, "openai-completions");
		assert_eq!(base.base_url, "https://api.fireworks.ai/inference/v1");
		assert_json_eq(&to_json(&base.compat), &compat);
		assert_json_eq(&to_json(&base.thinking_level_map), &thinking_level_map);
		assert_eq!(fast.api, base.api);
		assert_eq!(fast.base_url, base.base_url);
		assert_json_eq(&to_json(&fast.compat), &compat);
		assert_json_eq(&to_json(&fast.thinking_level_map), &thinking_level_map);
	}

	// Regression for #9323: native effort must reach Messages without budget-based fallback.
	#[test]
	fn sends_native_messages_effort_levels() {
		for (model_id, levels) in [
			(
				"accounts/fireworks/models/deepseek-v4p1-flash",
				vec![Off, Low, High, Max],
			),
			("accounts/fireworks/models/qwen3p8-max", vec![Off, Low, Medium, Xhigh]),
			(
				"accounts/fireworks/models/qwen3p8-2p4t-a95b",
				vec![Off, Low, Medium, Xhigh],
			),
		] {
			let model = fireworks(model_id);
			assert_eq!(model.api, "anthropic-messages", "{model_id}");
			assert_eq!(
				to_json(&model.compat)["forceAdaptiveThinking"],
				json!(true),
				"{model_id}"
			);
			assert_eq!(get_supported_thinking_levels(&model), levels, "{model_id}");
		}
	}

	// Regression for #9323: accepted aliases are not distinct native effort levels.
	#[test]
	fn exposes_distinct_native_effort_levels() {
		for model_id in [
			"accounts/fireworks/models/glm-5p3",
			"accounts/fireworks/routers/glm-5p3-fast",
			"accounts/fireworks/models/kimi-k3",
			"accounts/fireworks/routers/kimi-k3-fast",
		] {
			assert_eq!(
				get_supported_thinking_levels(&fireworks(model_id)),
				[Low, High, Max],
				"{model_id}"
			);
		}
	}

	#[test]
	fn keeps_toggle_only_messages_models_without_a_verified_fallback_on_budget_based_thinking() {
		let model = fireworks("accounts/fireworks/models/nemotron-3-ultra-nvfp4");
		assert_eq!(to_json(&model.compat).get("forceAdaptiveThinking"), None);
	}

	#[test]
	fn resolves_fireworks_api_key_from_the_environment() {
		pi_host::testing::run(|host| async move {
			host.set_env("FIREWORKS_API_KEY", "test-fireworks-key");

			assert_eq!(
				find_env_keys("fireworks", None),
				Some(vec!["FIREWORKS_API_KEY".to_string()])
			);
			assert_eq!(
				get_env_api_key("fireworks", None).as_deref(),
				Some("test-fireworks-key")
			);
		});
	}

	#[test]
	fn sets_fireworks_specific_compat_for_session_affinity_and_unsupported_tool_fields() {
		let compat = to_json(&fireworks("accounts/fireworks/models/nemotron-3-ultra-nvfp4").compat);

		assert!(compat.is_object());
		assert_eq!(compat["sendSessionAffinityHeaders"], json!(true));
		assert_eq!(compat["supportsEagerToolInputStreaming"], json!(false));
		assert_eq!(compat["supportsCacheControlOnTools"], json!(false));
		assert_eq!(compat["supportsLongCacheRetention"], json!(false));
		assert_eq!(compat["allowEmptySignature"], json!(true));
	}
}
