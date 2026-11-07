//! Port of `packages/ai/test/openrouter-reasoning-options.test.ts`.
//!
//! The `getOpenRouterThinkingLevelMap` describe block tests `catalog/sources/openrouter.ts`, a catalog build script that
//! has no Rust port, so it is not ported. The payload tests use the map that call returns for
//! `{ mandatory: true, supported_efforts: ["max", "high", "low"] }`, written out.

mod openai_completions_support;

use openai_completions_support::{capture_payload, context, model, user_message};
use pi_ai::api::openai_completions::stream_simple;
use pi_ai::types::{Model, SimpleStreamOptions, ThinkingLevel, ThinkingLevelMap};
use pi_host::testing::run;
use serde_json::{Value, json};

fn open_router_model(thinking_level_map: Option<ThinkingLevelMap>) -> Model {
	let mut model = model("stealth/ox-alpha", "openrouter", "https://example.invalid/v1");
	model.base.name = "Ox Alpha".into();
	model.reasoning = true;
	model.thinking_level_map = thinking_level_map;
	model.compat = json!({ "thinkingFormat": "openrouter" }).as_object().cloned();
	model
}

async fn payload_for(model: Model, reasoning: Option<ThinkingLevel>) -> Value {
	let (on_payload, payload) = capture_payload(Some("payload captured"));
	let mut options = SimpleStreamOptions::default();
	options.base.api_key = Some("test".into());
	options.base.on_payload = Some(on_payload);
	options.reasoning = reasoning;
	stream_simple(&model, &context(vec![user_message("Hello")]), Some(options))
		.unwrap()
		.result()
		.await;
	payload.borrow().clone().expect("OpenRouter payload was not captured")
}

fn mandatory_map() -> ThinkingLevelMap {
	serde_json::from_value(json!({
		"off": null,
		"minimal": null,
		"low": "low",
		"medium": null,
		"high": "high",
		"xhigh": null,
		"max": "max",
	}))
	.unwrap()
}

mod open_router_mandatory_reasoning_payloads {
	use super::*;

	#[test]
	fn omits_reasoning_when_a_background_call_does_not_request_it() {
		run(|_host| async move {
			let payload = payload_for(open_router_model(Some(mandatory_map())), None).await;
			assert!(!payload.as_object().unwrap().contains_key("reasoning"));
		});
	}

	#[test]
	fn still_sends_an_explicitly_selected_supported_effort() {
		run(|_host| async move {
			let payload = payload_for(open_router_model(Some(mandatory_map())), Some(ThinkingLevel::Low)).await;
			assert_eq!(payload["reasoning"]["effort"], json!("low"));
		});
	}

	#[test]
	fn continues_to_explicitly_disable_reasoning_for_optional_models() {
		run(|_host| async move {
			let payload = payload_for(open_router_model(None), None).await;
			assert_eq!(payload["reasoning"]["effort"], json!("none"));
		});
	}
}
