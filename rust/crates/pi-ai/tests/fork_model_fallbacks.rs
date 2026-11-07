//! Port of the Anthropic Messages case of `packages/ai/test/fork-model-fallbacks.test.ts`.
//!
//! `getForkChatModelFallbacks` lives in `packages/ai/scripts/` (catalog build tooling, not part of the Rust port),
//! so the test builds the Haiku 5.5 fallback from the literal in `scripts/fork-model-fallbacks.ts`, without the
//! `applyForkModelMetadata`/`applyImageInputMetadata` additions (input limits), which the payload does not read.
//! The other cases check the script and `hydrateModelCatalog` output and are not ported.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use pi_ai::api::anthropic_messages::{AnthropicOptions, stream};
use pi_ai::models::get_supported_thinking_levels;
use pi_ai::providers::model_schema::ModelThinkingLevel::{High, Low, Max, Medium, Off, Xhigh};
use pi_ai::types::{CacheRetention, Context, Model, StreamOptions};
use pi_ai::utils::transcript::normalize_context;
use pi_host::testing::run;
use pi_host::{FetchFunction, PiError};
use serde_json::{Value, json};

fn haiku_fallback() -> Model {
	serde_json::from_value(json!({
		"type": "chat",
		"id": "claude-haiku-5-5",
		"name": "Claude Haiku 5.5",
		"api": "anthropic-messages",
		"provider": "anthropic",
		"baseUrl": "https://api.anthropic.com",
		"reasoning": true,
		"thinkingLevelMap": { "minimal": null, "low": "low", "medium": "medium", "high": "high", "xhigh": "xhigh", "max": "max" },
		"input": ["text", "image"],
		"cost": {
			"input": 0.1,
			"output": 0.5,
			"cacheRead": 0.01,
			"cacheWrite": 0.125,
			"tiers": [{ "inputTokensAbove": 100000, "input": 0.5, "output": 2.5, "cacheRead": 0.05, "cacheWrite": 0.625 }],
		},
		"contextWindow": 1000000,
		"maxTokens": 128000,
		"compat": {
			"supportsMidConvoEffort": true,
			"supportsMidConvoSystemMessages": true,
			"supportsMidConvoToolChanges": true,
			"forceAdaptiveThinking": true,
			"supportsTemperature": false,
			"supportsStrictTools": true,
		},
		"promptCache": { "short": 300, "long": 3600 },
	}))
	.unwrap()
}

mod fork_chat_model_fallbacks {
	use super::*;

	// #10753
	#[test]
	fn disables_thinking_on_the_haiku_fallback_without_sending_effort_settings() {
		let model = haiku_fallback();
		assert_eq!(
			get_supported_thinking_levels(&model),
			[Off, Low, Medium, High, Xhigh, Max]
		);
		run(|_| async {
			let fetch_calls = Rc::new(Cell::new(0));
			let calls = fetch_calls.clone();
			let fetch: FetchFunction = Rc::new(move |_| {
				calls.set(calls.get() + 1);
				Box::pin(async { Err(PiError::new("Unexpected provider request")) })
			});
			let captured: Rc<RefCell<Option<Value>>> = Rc::default();
			let sink = captured.clone();
			let mut base = StreamOptions {
				cache_retention: Some(CacheRetention::None),
				..StreamOptions::default()
			};
			base.api_key = Some("test-key".into());
			base.fetch = Some(fetch);
			base.on_payload = Some(Rc::new(move |payload, _| {
				*sink.borrow_mut() = Some(payload);
				Box::pin(async { Err(PiError::new("Payload captured before network request")) })
			}));
			let options = AnthropicOptions {
				base,
				thinking_enabled: Some(false),
				..AnthropicOptions::default()
			};
			let context: Context =
				serde_json::from_value(json!({ "messages": [{ "role": "user", "content": "Hello", "timestamp": 0 }] }))
					.unwrap();
			let result = stream(&model, &normalize_context(&context), Some(options))
				.result()
				.await
				.unwrap();

			let payload = captured.borrow_mut().take().expect("payload captured");
			assert_eq!(payload["thinking"], json!({ "type": "disabled" }));
			assert_eq!(payload["messages"].as_array().unwrap().len(), 1);
			assert_eq!(payload["messages"][0]["role"], "user");
			assert_eq!(payload["messages"][0]["content"], "Hello");
			assert_eq!(payload.get("output_config"), None);
			assert_eq!(result.provider_thinking_level, None);
			assert!(
				result
					.error_message
					.as_deref()
					.unwrap()
					.contains("Payload captured before network request")
			);
			assert_eq!(fetch_calls.get(), 0);
		});
	}
}
