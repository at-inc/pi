//! Port of `packages/ai/test/anthropic-error-tool-result-images.test.ts`.
//!
//! No differences from the TS beyond Rust syntax: payloads are captured through the compat `streamSimple`.

mod anthropic_support;

use std::cell::RefCell;
use std::rc::Rc;

use pi_ai::compat::stream_simple;
use pi_ai::types::{CacheRetention, Context, Model, SimpleStreamOptions, StreamOptions};
use pi_host::PiError;
use pi_host::testing::run;
use serde_json::{Value, json};

/// The `model` of the TS file.
fn model() -> Model {
	serde_json::from_value(json!({
		"id": "claude-test",
		"name": "Claude Test",
		"api": "anthropic-messages",
		"provider": "anthropic",
		"baseUrl": "http://127.0.0.1:9",
		"reasoning": false,
		"input": ["text", "image"],
		"cost": { "input": 0, "output": 0, "cacheRead": 0, "cacheWrite": 0 },
		"contextWindow": 200000,
		"maxTokens": 1024,
	}))
	.unwrap()
}

/// The `assistant` of the TS file with the given tool calls.
fn assistant(content: Value) -> Value {
	json!({
		"role": "assistant",
		"content": content,
		"api": "anthropic-messages",
		"provider": "anthropic",
		"model": "claude-test",
		"usage": {
			"input": 0,
			"output": 0,
			"cacheRead": 0,
			"cacheWrite": 0,
			"totalTokens": 0,
			"cost": { "input": 0, "output": 0, "cacheRead": 0, "cacheWrite": 0, "total": 0 },
		},
		"stopReason": "toolUse",
		"timestamp": 1,
	})
}

fn codemode_call() -> Value {
	json!({ "type": "toolCall", "id": "call-1", "name": "codemode", "arguments": {} })
}

fn read_call() -> Value {
	json!({ "type": "toolCall", "id": "call-2", "name": "read", "arguments": {} })
}

/// `capturePayload(context)` with `cacheRetention: "none"`.
async fn capture(context: Value) -> Value {
	let context: Context = serde_json::from_value(context).unwrap();
	let options = SimpleStreamOptions {
		base: StreamOptions {
			cache_retention: Some(CacheRetention::None),
			..StreamOptions::default()
		},
		..SimpleStreamOptions::default()
	};
	capture_payload(&model(), &context, options).await
}

/// `capturePayload(model, context)` through the compat `streamSimple`: `onPayload` records the payload and throws.
async fn capture_payload(model: &Model, context: &Context, options: SimpleStreamOptions) -> Value {
	let captured: Rc<RefCell<Option<Value>>> = Rc::default();
	let recorder = captured.clone();
	let mut options = options;
	options.api_key = Some("fake-key".into());
	options.on_payload = Some(Rc::new(move |payload, _| {
		*recorder.borrow_mut() = Some(payload);
		Box::pin(async { Err(PiError::named("PayloadCaptured", "payload captured")) })
	}));
	stream_simple(model, context, Some(options)).unwrap().result().await;
	captured
		.borrow_mut()
		.take()
		.expect("Expected payload capture before request")
}

fn image(data: &str) -> Value {
	json!({ "type": "image", "source": { "type": "base64", "media_type": "image/png", "data": data } })
}

mod anthropic_error_tool_results_with_images {
	use super::*;

	// Regression for #10794: Anthropic rejects error tool results that contain images.
	#[test]
	fn moves_the_images_after_the_tool_results_of_the_turn() {
		run(|_| async {
			let payload = capture(json!({
				"messages": [
					{ "role": "user", "content": "Show the screenshots", "timestamp": 1 },
					assistant(json!([codemode_call(), read_call()])),
					{
						"role": "toolResult",
						"toolCallId": "call-1",
						"toolName": "codemode",
						"content": [
							{ "type": "text", "text": "Script failed" },
							{ "type": "image", "data": "AAAA", "mimeType": "image/png" },
							{ "type": "image", "data": "CCCC", "mimeType": "image/png" },
							{ "type": "text", "text": "Script error" },
						],
						"isError": true,
						"timestamp": 1,
					},
					{
						"role": "toolResult",
						"toolCallId": "call-2",
						"toolName": "read",
						"content": [{ "type": "image", "data": "BBBB", "mimeType": "image/png" }],
						"isError": false,
						"timestamp": 1,
					},
				],
			}))
			.await;

			assert_eq!(
				payload["messages"][2],
				json!({
					"role": "user",
					"content": [
						{
							"type": "tool_result",
							"tool_use_id": "call-1",
							"content": "Script failed\n(image attached after the tool results)\nScript error",
							"is_error": true,
						},
						{
							"type": "tool_result",
							"tool_use_id": "call-2",
							"content": [{ "type": "text", "text": "(see attached image)" }, image("BBBB")],
							"is_error": false,
						},
						{ "type": "text", "text": "Images from the error result of tool call call-1 (codemode):" },
						image("AAAA"),
						image("CCCC"),
					],
				})
			);
		});
	}

	#[test]
	fn keeps_error_tool_results_without_images_unchanged() {
		run(|_| async {
			let payload = capture(json!({
				"messages": [
					{ "role": "user", "content": "Run it", "timestamp": 1 },
					assistant(json!([codemode_call()])),
					{
						"role": "toolResult",
						"toolCallId": "call-1",
						"toolName": "codemode",
						"content": [{ "type": "text", "text": "Script failed" }],
						"isError": true,
						"timestamp": 1,
					},
				],
			}))
			.await;

			assert_eq!(
				payload["messages"][2],
				json!({
					"role": "user",
					"content": [{ "type": "tool_result", "tool_use_id": "call-1", "content": "Script failed", "is_error": true }],
				})
			);
		});
	}
}
