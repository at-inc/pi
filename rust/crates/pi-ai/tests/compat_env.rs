//! Port of `packages/ai/test/compat-env.test.ts`.
//!
//! Waits for the PORT-STUBs behind `compat/mod.rs`: the builtin API wrappers (`api/*_lazy.rs`, w3 shards), which
//! `resetApiProviders()` registers, and `utils/transcript.rs` and `providers/all.rs` (w2-ai-core), which `complete()`
//! calls.

use std::cell::RefCell;
use std::rc::Rc;

use pi_ai::compat::{ApiProvider, complete, register_api_provider, reset_api_providers};
use pi_ai::types::{
	AssistantMessage, AssistantMessageEvent, Context, Model, ProviderRequestOptions, ProviderStreamOptions,
	StreamOptions,
};
use pi_ai::utils::event_stream::AssistantMessageEventStream;
use pi_host::PiResult;
use serde_json::json;

fn context() -> Context {
	serde_json::from_value(json!({
		"messages": [{ "role": "user", "content": "hi", "timestamp": pi_host::now_ms() }],
	}))
	.unwrap()
}

fn model() -> Model {
	serde_json::from_value(json!({
		"id": "test-model",
		"name": "Test Model",
		"api": "openai-responses",
		"provider": "custom-openai",
		"baseUrl": "https://example.test/v1",
		"reasoning": false,
		"input": ["text"],
		"cost": { "input": 0, "output": 0, "cacheRead": 0, "cacheWrite": 0 },
		"contextWindow": 128000,
		"maxTokens": 4096,
	}))
	.unwrap()
}

fn message() -> AssistantMessage {
	let model = model();
	serde_json::from_value(json!({
		"role": "assistant",
		"content": [{ "type": "text", "text": "ok" }],
		"api": &*model.api,
		"provider": &*model.provider,
		"model": model.id,
		"usage": {
			"input": 0,
			"output": 0,
			"cacheRead": 0,
			"cacheWrite": 0,
			"totalTokens": 0,
			"cost": { "input": 0, "output": 0, "cacheRead": 0, "cacheWrite": 0, "total": 0 },
		},
		"stopReason": "stop",
		"timestamp": pi_host::now_ms(),
	}))
	.unwrap()
}

fn finished_stream() -> PiResult<AssistantMessageEventStream> {
	let stream = AssistantMessageEventStream::new();
	let output = message();
	stream
		.push(serde_json::from_value::<AssistantMessageEvent>(json!({ "type": "start", "partial": output })).unwrap());
	stream.push(
		serde_json::from_value::<AssistantMessageEvent>(json!({ "type": "done", "reason": "stop", "message": output }))
			.unwrap(),
	);
	stream.end(Some(output));
	Ok(stream)
}

/// `afterEach(() => resetApiProviders())`.
struct ResetApiProviders;

impl Drop for ResetApiProviders {
	fn drop(&mut self) {
		reset_api_providers();
	}
}

mod compat_legacy_api_fallback {
	use super::*;

	#[test]
	#[ignore = "PORT-STUB: needs api/*_lazy.rs from the w3 shards and utils/transcript.rs and providers/all.rs from w2-ai-core"]
	fn dispatches_unknown_providers_through_the_legacy_api_registry() {
		let _reset = ResetApiProviders;
		let captured_api_key: Rc<RefCell<Option<String>>> = Rc::default();
		let (stream_key, stream_simple_key) = (captured_api_key.clone(), captured_api_key.clone());
		register_api_provider(
			ApiProvider {
				api: "openai-responses".into(),
				stream: Rc::new(move |_model, _context, options| {
					*stream_key.borrow_mut() = options.and_then(|options| options.api_key.clone());
					finished_stream()
				}),
				stream_simple: Rc::new(move |_model, _context, options| {
					*stream_simple_key.borrow_mut() = options.and_then(|options| options.api_key.clone());
					finished_stream()
				}),
			},
			None,
		);

		let options = ProviderStreamOptions {
			base: StreamOptions {
				base: ProviderRequestOptions {
					api_key: Some("request-key".into()),
					..ProviderRequestOptions::default()
				},
				..StreamOptions::default()
			},
			..ProviderStreamOptions::default()
		};
		pi_host::testing::run(|_| complete(&model(), &context(), Some(options))).unwrap();

		assert_eq!(captured_api_key.borrow().as_deref(), Some("request-key"));
	}
}
