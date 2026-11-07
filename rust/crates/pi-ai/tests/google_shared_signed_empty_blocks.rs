//! Port of `packages/ai/test/google-shared-signed-empty-blocks.test.ts`.
//!
//! Models and contexts are built from the TS object literals as JSON. `Date.now()` is a fixed time.

// Gemini can attach `thoughtSignature` to a response part whose visible text is empty (e.g. a
// thought burst preceding a function call) and requires the signature echoed back on the next
// request. Dropping such blocks while rebuilding history silently breaks the reasoning chain:
// the model then intermittently ends a mid-task turn with a thought-only STOP and no tool call.
// These tests pin the rule: an empty text/thinking block is skipped only when it is UNSIGNED.

use pi_ai::api::google_shared::convert_messages;
use pi_ai::types::{Context, Model};
use pi_ai::utils::transcript::normalize_context;
use pi_host::js::json_stringify;
use serde_json::{Value, json};

const NOW: f64 = 1_700_000_000_000.0;
const VALID_SIG: &str = "AAAAAAAAAAAAAAAAAAAAAA==";

fn make_model() -> Model {
	serde_json::from_value(json!({
		"id": "gemini-3-pro-preview",
		"name": "gemini-3-pro-preview",
		"api": "google-generative-ai",
		"provider": "google",
		"baseUrl": "https://example.com",
		"reasoning": true,
		"input": ["text"],
		"cost": { "input": 0, "output": 0, "cacheRead": 0, "cacheWrite": 0 },
		"contextWindow": 128000,
		"maxTokens": 8192,
	}))
	.unwrap()
}

fn make_context(model_id: &str, content: Value) -> Context {
	serde_json::from_value(json!({
		"messages": [
			{ "role": "user", "content": "Hi", "timestamp": NOW },
			{
				"role": "assistant",
				"content": content,
				"api": "google-generative-ai",
				"provider": "google",
				"model": model_id,
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
		],
	}))
	.unwrap()
}

fn model_turn(model: &Model, context: &Context) -> Value {
	let contents = pi_host::testing::run(|_| async { convert_messages(model, &normalize_context(context)) });
	contents
		.into_iter()
		.find(|content| content["role"] == "model")
		.expect("model turn")
}

fn parts(turn: &Value) -> &Vec<Value> {
	turn["parts"].as_array().unwrap()
}

mod google_shared_convert_messages_signed_empty_blocks {
	use super::*;

	#[test]
	fn keeps_a_signed_empty_thinking_block_so_its_signature_is_echoed_back() {
		let model = make_model();
		let turn = model_turn(
			&model,
			&make_context(
				&model.id,
				json!([
					{ "type": "thinking", "thinking": "", "thinkingSignature": VALID_SIG },
					{ "type": "toolCall", "id": "call_1", "name": "bash", "arguments": { "command": "ls" } },
				]),
			),
		);
		let signed: Vec<&Value> = parts(&turn)
			.iter()
			.filter(|part| part["thoughtSignature"] == VALID_SIG)
			.collect();
		assert_eq!(signed.len(), 1);
		assert_eq!(signed[0]["thought"], true);
	}

	#[test]
	fn keeps_a_signed_empty_text_block_the_same_way() {
		let model = make_model();
		let turn = model_turn(
			&model,
			&make_context(
				&model.id,
				json!([
					{ "type": "text", "text": "", "textSignature": VALID_SIG },
					{ "type": "toolCall", "id": "call_1", "name": "bash", "arguments": { "command": "ls" } },
				]),
			),
		);
		let signed = parts(&turn)
			.iter()
			.filter(|part| part["thoughtSignature"] == VALID_SIG)
			.count();
		assert_eq!(signed, 1);
	}

	#[test]
	fn still_drops_unsigned_empty_blocks() {
		let model = make_model();
		let turn = model_turn(
			&model,
			&make_context(
				&model.id,
				json!([
					{ "type": "thinking", "thinking": "" },
					{ "type": "text", "text": "   " },
					{ "type": "toolCall", "id": "call_1", "name": "bash", "arguments": { "command": "ls" } },
				]),
			),
		);
		assert_eq!(parts(&turn).len(), 1);
		assert!(parts(&turn)[0].get("functionCall").is_some());
	}

	#[test]
	fn still_drops_signed_empty_blocks_from_a_different_provider_model_signature_unusable() {
		let model = make_model();
		let turn = model_turn(
			&model,
			&make_context(
				"other-model",
				json!([
					{ "type": "thinking", "thinking": "", "thinkingSignature": VALID_SIG },
					{ "type": "text", "text": "", "textSignature": VALID_SIG },
					{ "type": "toolCall", "id": "call_1", "name": "bash", "arguments": { "command": "ls" } },
				]),
			),
		);
		assert_eq!(parts(&turn).len(), 1);
		assert!(parts(&turn)[0].get("functionCall").is_some());
		assert!(!json_stringify(&turn).contains(VALID_SIG));
	}
}
