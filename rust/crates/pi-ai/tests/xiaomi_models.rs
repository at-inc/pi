//! Port of `packages/ai/test/xiaomi-models.test.ts`.
//!
//! `getModels` (compat.ts) is `get_builtin_models`. `it.each` cases run in a loop.

use std::cell::RefCell;
use std::rc::Rc;

use pi_ai::compat::stream_simple;
use pi_ai::models::get_supported_thinking_levels;
use pi_ai::providers::all::get_builtin_models;
use pi_ai::providers::model_schema::ModelThinkingLevel;
use pi_ai::types::{Context, Model, SimpleStreamOptions, ThinkingLevel};
use pi_host::PiError;
use serde_json::{Value, json};

const XIAOMI_PROVIDERS: [&str; 4] = [
	"xiaomi",
	"xiaomi-token-plan-cn",
	"xiaomi-token-plan-ams",
	"xiaomi-token-plan-sgp",
];
const DEPRECATED_MODEL_IDS: [&str; 3] = ["mimo-v2-flash", "mimo-v2-omni", "mimo-v2-pro"];
const REPLACEMENT_MODEL_IDS: [&str; 2] = ["mimo-v2.6-flash", "mimo-v2.6-pro"];

/// `capturePayload(reasoning)`: the payload `onPayload` captured before it threw.
async fn capture_payload(model: &Model, reasoning: Option<ThinkingLevel>) -> Value {
	let context: Context =
		serde_json::from_value(json!({ "messages": [{ "role": "user", "content": "test", "timestamp": 0 }] })).unwrap();
	let captured: Rc<RefCell<Option<Value>>> = Rc::default();
	let recorder = captured.clone();
	let mut options = SimpleStreamOptions {
		reasoning,
		..SimpleStreamOptions::default()
	};
	options.api_key = Some("test-xiaomi-key".into());
	options.on_payload = Some(Rc::new(move |payload, _| {
		*recorder.borrow_mut() = Some(payload);
		Box::pin(async { Err(PiError::new("payload captured")) })
	}));
	stream_simple(model, &context, Some(options)).unwrap().result().await;
	captured.borrow_mut().take().expect("payload captured")
}

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
				let label = format!("{provider}/{}", model.id);
				assert_eq!(
					get_supported_thinking_levels(&model),
					[ModelThinkingLevel::Off, ModelThinkingLevel::High],
					"{label}"
				);

				pi_host::testing::run(|_| async {
					let enabled = capture_payload(&model, Some(ThinkingLevel::High)).await;
					assert_eq!(enabled["thinking"], json!({ "type": "enabled" }), "{label}");
					assert!(enabled.get("reasoning_effort").is_none(), "{label}");

					let disabled = capture_payload(&model, None).await;
					assert_eq!(disabled["thinking"], json!({ "type": "disabled" }), "{label}");
					assert!(disabled.get("reasoning_effort").is_none(), "{label}");
				});
			}
		}
	}
}
