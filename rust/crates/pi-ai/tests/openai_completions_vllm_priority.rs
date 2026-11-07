//! Port of `packages/ai/test/openai-completions-vllm-priority.test.ts`.

mod openai_completions_support;

use openai_completions_support::{builtin_model, last_body, options, serve_chunks, user_message};
use pi_ai::api::openai_completions::stream;
use pi_ai::types::{Context, Model};
use pi_ai::utils::transcript::normalize_context;
use pi_host::testing::{TestHost, run};
use serde_json::{Value, json};

fn create_model(compat: Option<Value>) -> Model {
	let mut model = builtin_model("openai", "gpt-4o-mini");
	model.compat = compat.map(|compat| compat.as_object().cloned().expect("compat object"));
	model
}

async fn capture_request(host: &TestHost, model: Model) -> Value {
	serve_chunks(
		host,
		vec![json!({
			"choices": [{ "delta": {}, "finish_reason": "stop" }],
			"usage": {
				"prompt_tokens": 1,
				"completion_tokens": 1,
				"prompt_tokens_details": { "cached_tokens": 0 },
				"completion_tokens_details": { "reasoning_tokens": 0 },
			},
		})],
	);
	let context = normalize_context(&Context {
		system_prompt: Some("sys".into()),
		messages: vec![user_message("hi")],
		..Context::default()
	});
	stream(&model, &context, Some(options("test-key"))).result().await;
	last_body(host)
}

mod openai_completions_vllm_priority {
	use super::*;

	#[test]
	fn sends_compat_vllm_priority_as_the_top_level_priority_request_field() {
		run(|host| async move {
			let payload = capture_request(&host, create_model(Some(json!({ "vllmPriority": 10 })))).await;

			assert_eq!(payload["priority"], json!(10));
		});
	}

	#[test]
	fn omits_priority_when_vllm_priority_is_not_set() {
		run(|host| async move {
			let payload = capture_request(&host, create_model(None)).await;

			assert_eq!(payload.get("priority"), None);
		});
	}
}
