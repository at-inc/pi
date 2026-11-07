//! Port of `packages/ai/test/anthropic-eager-tool-input-compat.test.ts`.
//!
//! Differences from the TS: the local `node:http` server is the test host's fetch, which records each request and
//! answers it with an empty `text/event-stream` response. Header lookups are case-insensitive, like Node's
//! lowercased `request.headers`. The TS reads the stream until `done` or `error`; here the test awaits `result()`,
//! which ends at the same event.

mod anthropic_support;

use anthropic_support::context;
use indexmap::IndexMap;
use pi_ai::api::anthropic_messages::{AnthropicOptions, stream};
use pi_ai::types::{CacheRetention, Model, StreamOptions};
use pi_host::HttpResponse;
use pi_host::testing::{RecordedRequest, run};
use serde_json::{Map, Value, json};

/// `createModel(baseUrl, compat)`.
fn create_model(base_url: &str, compat: Option<Value>) -> Model {
	let mut merged = Map::new();
	merged.insert("forceAdaptiveThinking".into(), json!(true));
	if let Some(Value::Object(compat)) = compat {
		merged.extend(compat);
	}
	serde_json::from_value(json!({
		"id": "claude-opus-4-8",
		"name": "Claude Opus 4.8",
		"api": "anthropic-messages",
		"provider": "test-anthropic",
		"baseUrl": base_url,
		"reasoning": true,
		"input": ["text"],
		"cost": { "input": 0, "output": 0, "cacheRead": 0, "cacheWrite": 0 },
		"contextWindow": 200000,
		"maxTokens": 32000,
		"compat": merged,
	}))
	.unwrap()
}

/// The `tool` of the TS file: `Type.Object({ value: Type.String() })` as its JSON Schema.
fn tool() -> Value {
	json!({
		"name": "lookup",
		"description": "Look up a value",
		"parameters": { "type": "object", "required": ["value"], "properties": { "value": { "type": "string" } } },
	})
}

/// `createContext(tools)`.
fn create_context(tools: Vec<Value>) -> Value {
	let mut context = Map::new();
	context.insert(
		"messages".into(),
		json!([{ "role": "user", "content": "Use the tool", "timestamp": 1 }]),
	);
	if !tools.is_empty() {
		context.insert("tools".into(), Value::Array(tools));
	}
	Value::Object(context)
}

/// `captureAnthropicRequest(compat, context)`.
async fn capture_anthropic_request(
	host: &pi_host::testing::TestHost,
	compat: Option<Value>,
	context_value: Value,
) -> RecordedRequest {
	host.on_fetch(|_| {
		Box::pin(async {
			Ok(HttpResponse::from_bytes(
				200,
				IndexMap::from([("content-type".to_string(), "text/event-stream".to_string())]),
				Vec::<u8>::new(),
			))
		})
	});
	let options = AnthropicOptions {
		base: StreamOptions {
			base: pi_ai::types::ProviderRequestOptions {
				api_key: Some("test-key".into()),
				..Default::default()
			},
			cache_retention: Some(CacheRetention::None),
			..StreamOptions::default()
		},
		..AnthropicOptions::default()
	};
	stream(
		&create_model("http://127.0.0.1:0", compat),
		&context(context_value),
		Some(options),
	)
	.result()
	.await;

	host.requests().pop().expect("Anthropic request was not captured")
}

/// `getFirstTool(body)`.
fn get_first_tool(body: &Value) -> Map<String, Value> {
	body.get("tools")
		.and_then(Value::as_array)
		.and_then(|tools| tools.first())
		.and_then(Value::as_object)
		.cloned()
		.expect("Expected first tool in request body")
}

mod anthropic_eager_tool_input_streaming_compatibility {
	use super::*;

	#[test]
	fn sends_per_tool_eager_input_streaming_by_default() {
		run(|host| async move {
			let request = capture_anthropic_request(&host, None, create_context(vec![tool()])).await;

			assert_eq!(
				get_first_tool(&request.json()).get("eager_input_streaming"),
				Some(&json!(true))
			);
			assert_eq!(request.header("anthropic-beta"), None);
		});
	}

	#[test]
	fn uses_the_legacy_fine_grained_tool_streaming_beta_when_eager_tool_input_streaming_is_disabled() {
		run(|host| async move {
			let request = capture_anthropic_request(
				&host,
				Some(json!({ "supportsEagerToolInputStreaming": false })),
				create_context(vec![tool()]),
			)
			.await;

			assert_eq!(get_first_tool(&request.json()).get("eager_input_streaming"), None);
			assert_eq!(
				request.header("anthropic-beta"),
				Some("fine-grained-tool-streaming-2025-05-14")
			);
		});
	}

	#[test]
	fn does_not_send_the_legacy_fine_grained_tool_streaming_beta_when_there_are_no_tools() {
		run(|host| async move {
			let request = capture_anthropic_request(
				&host,
				Some(json!({ "supportsEagerToolInputStreaming": false })),
				create_context(vec![]),
			)
			.await;

			assert_eq!(request.json().get("tools"), None);
			assert_eq!(request.header("anthropic-beta"), None);
		});
	}
}
