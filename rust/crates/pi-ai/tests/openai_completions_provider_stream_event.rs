//! Port of `packages/ai/test/openai-completions-provider-stream-event.test.ts`.
//!
//! The observed chunks are the SDK's parsed SSE events, so they equal the chunks the mocked stream yields.

mod openai_completions_support;

use std::cell::RefCell;
use std::rc::Rc;

use openai_completions_support::{model, serve_chunks, user_message};
use pi_ai::compat::complete_simple;
use pi_ai::types::{
	AssistantContent, Context, Model, OnProviderStreamEvent, SimpleStreamOptions, TextContent, UsageCost,
};
use pi_host::testing::run;
use serde_json::{Value, json};

fn open_router_model(provider: &str) -> Model {
	let mut model = model("openrouter/auto", provider, "https://openrouter.ai/api/v1");
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

fn api_key_options() -> SimpleStreamOptions {
	let mut options = SimpleStreamOptions::default();
	options.base.api_key = Some("test".into());
	options
}

mod openai_completions_provider_stream_events {
	use super::*;

	// Regression test for #9784.
	#[test]
	fn exposes_provider_chunks_including_openrouter_metadata() {
		run(|host| async move {
			let first_chunk = json!({
				"id": "chatcmpl-1",
				"model": "anthropic/claude-sonnet-4.6",
				"choices": [{ "index": 0, "delta": { "content": "hello" } }],
			});
			let final_chunk = json!({
				"id": "chatcmpl-1",
				"model": "anthropic/claude-sonnet-4.6",
				"choices": [{ "index": 0, "delta": {}, "finish_reason": "stop" }],
				"usage": {
					"prompt_tokens": 10,
					"completion_tokens": 2,
					"total_tokens": 12,
					"cost": 0.0012,
					"is_byok": false,
				},
				"openrouter_metadata": { "strategy": "direct", "region": "iad" },
			});
			serve_chunks(&host, vec![first_chunk.clone(), final_chunk.clone()]);
			let events: Rc<RefCell<Vec<Value>>> = Rc::default();
			let recorded = events.clone();
			let on_provider_stream_event: OnProviderStreamEvent = Rc::new(move |data, _model| {
				recorded.borrow_mut().push(data.clone());
				Box::pin(async { Ok(()) })
			});
			let mut options = api_key_options();
			options.on_provider_stream_event = Some(on_provider_stream_event);

			let message = complete_simple(&open_router_model("openrouter"), &hi_context(), Some(options))
				.await
				.unwrap()
				.unwrap();

			assert_eq!(
				message.content,
				vec![AssistantContent::Text(TextContent {
					text: "hello".into(),
					..TextContent::default()
				})]
			);
			assert_eq!(*events.borrow(), vec![first_chunk, final_chunk]);
		});
	}

	// Regression test for #9980.
	#[test]
	fn uses_openrouters_reported_total_without_double_counting_non_byok_upstream_cost() {
		run(|host| async move {
			serve_chunks(
				&host,
				vec![json!({
					"id": "chatcmpl-cost",
					"model": "openrouter/auto",
					"choices": [{ "index": 0, "delta": { "content": "done" }, "finish_reason": "stop" }],
					// These billing fields were captured from a production OpenRouter stream
					// even though the OpenAI SDK omits them from CompletionUsage.
					"usage": {
						"prompt_tokens": 100,
						"completion_tokens": 20,
						"cost": 0.004,
						"is_byok": false,
						"cost_details": { "upstream_inference_cost": 0.003 },
					},
				})],
			);

			let message = complete_simple(
				&open_router_model("openrouter-work"),
				&hi_context(),
				Some(api_key_options()),
			)
			.await
			.unwrap()
			.unwrap();

			assert_eq!(
				message.usage.cost,
				UsageCost {
					total: 0.004,
					total_from_provider: Some(true),
					..UsageCost::default()
				}
			);
		});
	}

	#[test]
	fn includes_upstream_inference_cost_for_openrouter_byok_requests() {
		run(|host| async move {
			serve_chunks(
				&host,
				vec![json!({
					"id": "chatcmpl-byok",
					"model": "openrouter/auto",
					"choices": [{ "index": 0, "delta": { "content": "done" }, "finish_reason": "stop" }],
					"usage": {
						"prompt_tokens": 100,
						"completion_tokens": 20,
						"cost": 0.0002,
						"is_byok": true,
						"cost_details": { "upstream_inference_cost": 0.003 },
					},
				})],
			);

			let message = complete_simple(&open_router_model("openrouter"), &hi_context(), Some(api_key_options()))
				.await
				.unwrap()
				.unwrap();

			assert!((message.usage.cost.total - 0.0032).abs() < 5e-11);
			assert_eq!(message.usage.cost.total_from_provider, Some(true));
		});
	}
}
