//! Port of the `anthropic-messages` case of `packages/ai/test/sampling-options.test.ts`.
//!
//! Every other case streams through the OpenAI Completions, Responses, Azure or Codex APIs and moves with those
//! shards.

use std::cell::RefCell;
use std::rc::Rc;

use pi_ai::compat::stream;
use pi_ai::types::{Context, Model, ProviderStreamOptions, StreamOptions};
use pi_host::PiError;
use pi_host::testing::run;
use serde_json::{Map, Value, json};

fn make_context() -> Context {
	serde_json::from_value(json!({ "messages": [{ "role": "user", "content": "Hello", "timestamp": 1 }] })).unwrap()
}

fn make_model(api: &str) -> Model {
	serde_json::from_value(json!({
		"id": "custom-model",
		"name": "Custom Model",
		"api": api,
		"provider": "custom-provider",
		"baseUrl": "http://127.0.0.1:9/v1",
		"reasoning": false,
		"input": ["text"],
		"cost": { "input": 0, "output": 0, "cacheRead": 0, "cacheWrite": 0 },
		"contextWindow": 128000,
		"maxTokens": 16384,
	}))
	.unwrap()
}

async fn capture_payload(model: &Model, mut base: StreamOptions) -> Value {
	let captured: Rc<RefCell<Option<Value>>> = Rc::default();
	let sink = captured.clone();
	base.api_key = Some("fake-key".into());
	base.on_payload = Some(Rc::new(move |payload, _| {
		*sink.borrow_mut() = Some(payload);
		Box::pin(async { Err(PiError::new("payload captured")) })
	}));
	let options = ProviderStreamOptions {
		base,
		extra: Map::new(),
	};
	stream(model, &make_context(), Some(options)).unwrap().result().await;
	captured
		.borrow_mut()
		.take()
		.expect("Expected payload to be captured before request failure")
}

mod sampling_params {
	use super::*;

	#[test]
	fn is_ignored_by_non_openai_compatible_apis() {
		run(|_| async {
			let options = StreamOptions {
				sampling_params: Some(json!({ "top_p": 0.9, "top_k": 40 }).as_object().unwrap().clone()),
				..StreamOptions::default()
			};
			let payload = capture_payload(&make_model("anthropic-messages"), options).await;
			assert!(payload.get("top_p").is_none());
			assert!(payload.get("top_k").is_none());
		});
	}
}
