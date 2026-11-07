//! Port of `packages/ai/test/azure-openai-tool-choice.test.ts`.

mod openai_responses_support;

use std::cell::RefCell;
use std::rc::Rc;

use openai_responses_support::{assert_match_object, context, model};
use pi_ai::api::azure_openai_responses::{AzureOpenAIResponsesOptions, stream, stream_simple};
use pi_ai::types::{Model, OnPayload, SimpleStreamOptions, ToolChoice, TranscriptContext};
use pi_host::PiError;
use pi_host::testing::run;
use serde_json::{Value, json};

fn azure_model() -> Model {
	model(json!({
		"id": "test-deployment",
		"name": "Test Deployment",
		"api": "azure-openai-responses",
		"provider": "azure",
		"baseUrl": "http://127.0.0.1:9/openai/v1",
		"reasoning": false,
		"input": ["text"],
		"cost": { "input": 0, "output": 0, "cacheRead": 0, "cacheWrite": 0 },
		"contextWindow": 10_000,
		"maxTokens": 1_000,
	}))
}

fn summarize_context() -> TranscriptContext {
	context(json!({
		"messages": [{ "role": "user", "content": "Summarize this", "timestamp": 1 }],
		"tools": [{
			"name": "read",
			"description": "Read a file",
			"parameters": { "type": "object", "properties": { "path": { "type": "string" } }, "required": ["path"] },
		}],
	}))
}

/// `onPayload: (requestPayload) => { payload = requestPayload; throw new Error("payload captured"); }`
fn capture_and_throw() -> (OnPayload, Rc<RefCell<Option<Value>>>) {
	let captured = Rc::new(RefCell::new(None));
	let target = captured.clone();
	let on_payload: OnPayload = Rc::new(move |params, _model| {
		*target.borrow_mut() = Some(params);
		Box::pin(async { Err(PiError::new("payload captured")) })
	});
	(on_payload, captured)
}

mod azure_open_ai_tool_choice {
	use super::*;

	#[test]
	fn forwards_provider_specific_tool_choice_while_preserving_tool_definitions() {
		run(|_| async {
			let (on_payload, payload) = capture_and_throw();
			let mut options = AzureOpenAIResponsesOptions {
				tool_choice: Some(json!("required")),
				..AzureOpenAIResponsesOptions::default()
			};
			options.api_key = Some("test-key".into());
			options.on_payload = Some(on_payload);
			let result = stream(&azure_model(), &summarize_context(), Some(options));

			result.result().await;

			let payload = payload.borrow_mut().take().expect("payload was captured");
			assert_match_object(&payload, &json!({ "tool_choice": "required" }));
			assert_eq!(payload["tools"].as_array().map(Vec::len), Some(1));
		});
	}

	#[test]
	fn forwards_provider_neutral_tool_choice_from_simple_options() {
		run(|_| async {
			let (on_payload, payload) = capture_and_throw();
			let mut options = SimpleStreamOptions {
				tool_choice: Some(ToolChoice::None),
				..SimpleStreamOptions::default()
			};
			options.api_key = Some("test-key".into());
			options.on_payload = Some(on_payload);
			let result = stream_simple(&azure_model(), &summarize_context(), Some(options)).expect("stream starts");

			result.result().await;

			let payload = payload.borrow_mut().take().expect("payload was captured");
			assert_match_object(&payload, &json!({ "tool_choice": "none" }));
			assert_eq!(payload["tools"].as_array().map(Vec::len), Some(1));
		});
	}
}
