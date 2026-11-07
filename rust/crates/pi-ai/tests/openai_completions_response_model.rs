//! Port of `packages/ai/test/openai-completions-response-model.test.ts`.

mod openai_completions_support;

use openai_completions_support::{model, serve_chunks, user_message};
use pi_ai::compat::complete;
use pi_ai::types::{Context, Model, ProviderStreamOptions, StopReason};
use pi_host::testing::run;
use serde_json::json;

fn open_router_auto() -> Model {
	let mut model = model("openrouter/auto", "openrouter", "https://openrouter.ai/api/v1");
	model.base.name = "OpenRouter Auto".into();
	model.context_window = 200_000.0;
	model.max_tokens = 8192.0;
	model
}

fn hi_context() -> Context {
	Context {
		messages: vec![user_message("hi")],
		..Context::default()
	}
}

fn api_key_options() -> Option<ProviderStreamOptions> {
	let mut options = ProviderStreamOptions::default();
	options.base.api_key = Some("test".into());
	Some(options)
}

mod openai_completions_response_model {
	use super::*;

	#[test]
	fn surfaces_routed_chunk_model_on_response_model_without_changing_model() {
		run(|host| async move {
			serve_chunks(
				&host,
				vec![
					json!({ "id": "chatcmpl-1", "model": "anthropic/claude-opus-4.8", "choices": [{ "index": 0, "delta": { "content": "hi" } }] }),
					json!({
						"id": "chatcmpl-1",
						"model": "anthropic/claude-opus-4.8",
						"choices": [{ "index": 0, "delta": {}, "finish_reason": "stop" }],
						"usage": {
							"prompt_tokens": 10,
							"completion_tokens": 5,
							"prompt_tokens_details": { "cached_tokens": 0 },
							"completion_tokens_details": { "reasoning_tokens": 0 },
						},
					}),
				],
			);

			let message = complete(&open_router_auto(), &hi_context(), api_key_options())
				.await
				.unwrap()
				.unwrap();

			assert_eq!(message.model, "openrouter/auto");
			assert_eq!(message.response_model.as_deref(), Some("anthropic/claude-opus-4.8"));
			assert_eq!(message.provider, "openrouter");
			assert_eq!(message.stop_reason, StopReason::Stop);
		});
	}

	#[test]
	fn leaves_response_model_undefined_when_chunks_echo_the_requested_id() {
		run(|host| async move {
			serve_chunks(
				&host,
				vec![
					json!({ "id": "chatcmpl-2", "model": "openrouter/auto", "choices": [{ "index": 0, "delta": { "content": "hi" } }] }),
					json!({
						"id": "chatcmpl-2",
						"model": "openrouter/auto",
						"choices": [{ "index": 0, "delta": {}, "finish_reason": "stop" }],
						"usage": {
							"prompt_tokens": 1,
							"completion_tokens": 1,
							"prompt_tokens_details": { "cached_tokens": 0 },
							"completion_tokens_details": { "reasoning_tokens": 0 },
						},
					}),
				],
			);

			let message = complete(&open_router_auto(), &hi_context(), api_key_options())
				.await
				.unwrap()
				.unwrap();

			assert_eq!(message.model, "openrouter/auto");
			assert_eq!(message.response_model, None);
		});
	}

	#[test]
	fn ignores_empty_or_missing_chunk_model() {
		run(|host| async move {
			serve_chunks(
				&host,
				vec![
					json!({ "id": "chatcmpl-3", "choices": [{ "index": 0, "delta": { "content": "hi" } }] }),
					json!({ "id": "chatcmpl-3", "model": "", "choices": [{ "index": 0, "delta": { "content": "!" } }] }),
					json!({
						"id": "chatcmpl-3",
						"choices": [{ "index": 0, "delta": {}, "finish_reason": "stop" }],
						"usage": {
							"prompt_tokens": 1,
							"completion_tokens": 2,
							"prompt_tokens_details": { "cached_tokens": 0 },
							"completion_tokens_details": { "reasoning_tokens": 0 },
						},
					}),
				],
			);

			let message = complete(&open_router_auto(), &hi_context(), api_key_options())
				.await
				.unwrap()
				.unwrap();

			assert_eq!(message.model, "openrouter/auto");
			assert_eq!(message.response_model, None);
		});
	}
}
