//! Port of `packages/ai/test/xiaomi-models.test.ts`.
//!
//! `getModels` (compat.ts) is `get_builtin_models`. Not ported: the payload half of
//! `models %s reasoning as an off/on toggle`, which captures the request through `streamSimple` on the
//! openai-completions API (wave-3, `w3-openai-completions`). The thinking-level half is ported.

use pi_ai::models::get_supported_thinking_levels;
use pi_ai::providers::all::get_builtin_models;
use pi_ai::providers::model_schema::ModelThinkingLevel;

const XIAOMI_PROVIDERS: [&str; 4] = [
	"xiaomi",
	"xiaomi-token-plan-cn",
	"xiaomi-token-plan-ams",
	"xiaomi-token-plan-sgp",
];
const DEPRECATED_MODEL_IDS: [&str; 3] = ["mimo-v2-flash", "mimo-v2-omni", "mimo-v2-pro"];
const REPLACEMENT_MODEL_IDS: [&str; 2] = ["mimo-v2.6-flash", "mimo-v2.6-pro"];

fn model_ids(provider: &str) -> Vec<String> {
	get_builtin_models(provider)
		.into_iter()
		.map(|model| model.id.clone())
		.collect()
}

mod xiaomi_mi_mo_models {
	use super::*;

	#[test]
	fn omits_deprecated_models() {
		for provider in XIAOMI_PROVIDERS {
			let model_ids = model_ids(provider);
			for model_id in DEPRECATED_MODEL_IDS {
				assert!(
					!model_ids.iter().any(|id| id == model_id),
					"{provider} lists {model_id}"
				);
			}
		}
	}

	#[test]
	fn keeps_replacement_models() {
		for provider in XIAOMI_PROVIDERS {
			let model_ids = model_ids(provider);
			for model_id in REPLACEMENT_MODEL_IDS {
				assert!(
					model_ids.iter().any(|id| id == model_id),
					"{provider} misses {model_id}"
				);
			}
		}
	}

	// #10818: MiMo Chat Completions only supports a thinking on/off toggle.
	#[test]
	fn models_reasoning_as_an_off_on_toggle() {
		for provider in XIAOMI_PROVIDERS {
			let models: Vec<_> = get_builtin_models(provider)
				.into_iter()
				.filter(|model| model.reasoning)
				.collect();
			assert!(!models.is_empty(), "{provider}");

			for model in models {
				assert_eq!(
					get_supported_thinking_levels(&model),
					[ModelThinkingLevel::Off, ModelThinkingLevel::High],
					"{provider}/{}",
					model.id
				);
			}
		}
	}
}
