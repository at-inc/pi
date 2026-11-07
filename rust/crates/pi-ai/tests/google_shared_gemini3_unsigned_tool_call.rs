//! Port of `packages/ai/test/google-shared-gemini3-unsigned-tool-call.test.ts`.
//!
//! Models and contexts are built from the TS object literals as JSON. `Date.now()` is a fixed time. `it.each` cases run
//! in a loop.

use pi_ai::api::google_shared::{convert_messages, requires_tool_call_id};
use pi_ai::types::{Context, Model};
use pi_ai::utils::transcript::normalize_context;
use pi_host::js::json_stringify;
use serde_json::{Value, json};

const NOW: f64 = 1_700_000_000_000.0;

fn make_gemini3_model(api: &str, provider: &str, id: &str) -> Model {
	serde_json::from_value(json!({
		"id": id,
		"name": "Gemini 3 Pro Preview",
		"api": api,
		"provider": provider,
		"baseUrl": "https://example.com",
		"reasoning": true,
		"input": ["text"],
		"cost": { "input": 0, "output": 0, "cacheRead": 0, "cacheWrite": 0 },
		"contextWindow": 128000,
		"maxTokens": 8192,
	}))
	.unwrap()
}

fn make_context(api: &str, provider: &str, id: &str, thought_signature: Option<&str>) -> Context {
	let mut first_call = json!({
		"type": "toolCall",
		"id": "call_1",
		"name": "bash",
		"arguments": { "command": "echo hi" },
	});
	if let Some(thought_signature) = thought_signature {
		first_call["thoughtSignature"] = json!(thought_signature);
	}
	serde_json::from_value(json!({
		"messages": [
			{ "role": "user", "content": "Hi", "timestamp": NOW },
			{
				"role": "assistant",
				"content": [
					first_call,
					{ "type": "toolCall", "id": "call_2", "name": "bash", "arguments": { "command": "ls -la" } },
				],
				"api": api,
				"provider": provider,
				"model": id,
				"usage": {
					"input": 0,
					"output": 0,
					"cacheRead": 0,
					"cacheWrite": 0,
					"totalTokens": 0,
					"cost": { "input": 0, "output": 0, "cacheRead": 0, "cacheWrite": 0, "total": 0 },
				},
				"stopReason": "toolUse",
				"timestamp": NOW,
			},
			{
				"role": "toolResult",
				"toolCallId": "call_1",
				"toolName": "bash",
				"content": [{ "type": "text", "text": "hi" }],
				"isError": false,
				"timestamp": NOW,
			},
			{
				"role": "toolResult",
				"toolCallId": "call_2",
				"toolName": "bash",
				"content": [{ "type": "text", "text": "files" }],
				"isError": false,
				"timestamp": NOW,
			},
		],
	}))
	.unwrap()
}

fn convert(model: &Model, context: &Context) -> Vec<Value> {
	pi_host::testing::run(|_| async { convert_messages(model, &normalize_context(context)) })
}

fn all_parts(contents: &[Value]) -> Vec<Value> {
	contents
		.iter()
		.flat_map(|content| content["parts"].as_array().cloned().unwrap_or_default())
		.collect()
}

fn model_turn(contents: &[Value]) -> Value {
	contents
		.iter()
		.find(|content| content["role"] == "model")
		.cloned()
		.expect("model turn")
}

fn function_call_parts(turn: &Value) -> Vec<Value> {
	turn["parts"]
		.as_array()
		.unwrap()
		.iter()
		.filter(|part| part.get("functionCall").is_some())
		.cloned()
		.collect()
}

mod google_shared_convert_messages_gemini_3_unsigned_tool_calls {
	use super::*;

	#[test]
	fn preserves_tool_call_ids_for_id_via_api_history() {
		for (api, provider, id) in [
			("google-generative-ai", "google", "gemini-3-pro-preview"),
			("google-generative-ai", "google", "gemini-3.6-flash"),
			("google-vertex", "google-vertex", "gemini-3-pro-preview"),
		] {
			let model = make_gemini3_model(api, provider, id);
			let contents = convert(&model, &make_context(api, provider, id, None));
			let parts = all_parts(&contents);
			let function_call_ids: Vec<&Value> = parts
				.iter()
				.filter_map(|part| part.pointer("/functionCall/id"))
				.collect();
			let function_response_ids: Vec<&Value> = parts
				.iter()
				.filter_map(|part| part.pointer("/functionResponse/id"))
				.collect();

			assert_eq!(function_call_ids, [&json!("call_1"), &json!("call_2")], "{id}");
			assert_eq!(function_response_ids, [&json!("call_1"), &json!("call_2")], "{id}");
		}
	}

	#[test]
	fn does_not_add_skip_thought_signature_validator_for_unsigned_google_gen_ai_tool_calls() {
		let model = make_gemini3_model("google-generative-ai", "google", "gemini-3-pro-preview");
		let contents = convert(
			&model,
			&make_context("google-generative-ai", "google", "other-model", None),
		);

		let turn = model_turn(&contents);
		let function_call_parts = function_call_parts(&turn);
		assert_eq!(function_call_parts.len(), 2);
		assert!(function_call_parts[0].get("thoughtSignature").is_none());
		assert!(function_call_parts[1].get("thoughtSignature").is_none());
		assert!(!json_stringify(&turn).contains("skip_thought_signature_validator"));

		let historical_text = turn["parts"]
			.as_array()
			.unwrap()
			.iter()
			.filter(|part| {
				part["text"]
					.as_str()
					.is_some_and(|text| text.contains("Historical context"))
			})
			.count();
		assert_eq!(historical_text, 0);
	}

	#[test]
	fn does_not_add_skip_thought_signature_validator_for_unsigned_vertex_tool_calls() {
		let model = make_gemini3_model("google-vertex", "google-vertex", "gemini-3-pro-preview");
		let contents = convert(
			&model,
			&make_context("google-vertex", "google-vertex", "gemini-3-pro-preview", None),
		);
		let turn = model_turn(&contents);
		let function_call_parts = function_call_parts(&turn);

		assert_eq!(function_call_parts.len(), 2);
		assert!(function_call_parts[0].get("thoughtSignature").is_none());
		assert!(function_call_parts[1].get("thoughtSignature").is_none());
		assert!(!json_stringify(&turn).contains("skip_thought_signature_validator"));
	}

	#[test]
	fn preserves_valid_thought_signature_when_present_for_the_same_provider_and_model() {
		let model = make_gemini3_model("google-generative-ai", "google", "gemini-3-pro-preview");
		let valid_sig = "AAAAAAAAAAAAAAAAAAAAAA==";
		let contents = convert(
			&model,
			&make_context(
				"google-generative-ai",
				"google",
				"gemini-3-pro-preview",
				Some(valid_sig),
			),
		);
		let function_call_parts = function_call_parts(&model_turn(&contents));

		assert_eq!(function_call_parts.len(), 2);
		assert_eq!(function_call_parts[0]["thoughtSignature"], valid_sig);
		assert!(function_call_parts[1].get("thoughtSignature").is_none());
	}

	#[test]
	fn does_not_add_a_thought_signature_for_non_gemini_3_models() {
		let model = make_gemini3_model("google-generative-ai", "google", "gemini-2.5-flash");
		let contents = convert(
			&model,
			&make_context("google-generative-ai", "google", "other-model", None),
		);
		let function_call_parts = function_call_parts(&model_turn(&contents));
		let function_response_parts: Vec<Value> = all_parts(&contents)
			.into_iter()
			.filter(|part| part.get("functionResponse").is_some())
			.collect();

		assert_eq!(function_call_parts.len(), 2);
		assert!(
			function_call_parts
				.iter()
				.all(|part| part.pointer("/functionCall/id").is_none())
		);
		assert!(
			function_call_parts
				.iter()
				.all(|part| part.get("thoughtSignature").is_none())
		);
		assert_eq!(function_response_parts.len(), 2);
		assert!(
			function_response_parts
				.iter()
				.all(|part| part.pointer("/functionResponse/id").is_none())
		);
	}
}

mod requires_tool_call_id {
	use super::*;

	#[test]
	fn returns_expected_for_model_id() {
		for (expected, model_id) in [
			(false, "gemini-2.5-flash"),
			(true, "gemini-3.6-flash"),
			(true, "claude-sonnet-4-5"),
			(true, "gpt-oss-120b"),
		] {
			assert_eq!(requires_tool_call_id(model_id), expected, "{model_id}");
		}
	}
}
