//! Port of `packages/ai/test/transform-messages-cross-model-thinking.test.ts`.
//!
//! Models and messages are built from the TS object literals as JSON. `Date.now()` timestamps are a fixed time.

use pi_ai::api::transform_messages::transform_messages;
use pi_ai::types::{Message, Model};
use serde_json::{Value, json};

const NOW: f64 = 1_700_000_000_000.0;

fn kimi() -> Model {
	serde_json::from_value(json!({
		"id": "moonshotai/Kimi-K3",
		"name": "Kimi K3",
		"api": "openai-completions",
		"provider": "sference",
		"baseUrl": "https://api.sference.com/v1",
		"reasoning": true,
		"input": ["text", "image"],
		"cost": { "input": 0, "output": 0, "cacheRead": 0, "cacheWrite": 0 },
		"contextWindow": 1048576,
		"maxTokens": 32768,
	}))
	.unwrap()
}

fn assistant(model: &str, content: Value) -> Value {
	json!({
		"role": "assistant",
		"content": content,
		"api": "openai-completions",
		"provider": "sference",
		"model": model,
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
	})
}

fn transform(messages: Vec<Value>) -> Vec<Message> {
	let messages: Vec<Message> = serde_json::from_value(Value::Array(messages)).unwrap();
	pi_host::testing::run(|_| async { transform_messages(&messages, &kimi(), None) })
}

fn assistant_contents(result: &[Message]) -> Vec<Value> {
	result
		.iter()
		.filter_map(|m| match m {
			Message::Assistant(assistant) => Some(serde_json::to_value(&assistant.content).unwrap()),
			_ => None,
		})
		.collect()
}

mod thinking_from_other_models {
	use super::*;

	// https://github.com/earendil-works/pi/issues/10995
	#[test]
	fn does_not_send_another_models_readable_thinking_as_text() {
		let result = transform(vec![
			json!({ "role": "user", "content": "Why does curl work but node fails?", "timestamp": NOW }),
			assistant(
				"zai-org/GLM-5.3",
				json!([
					{
						"type": "thinking",
						"thinking": "The user asks about ECONNRESET. Candidates: proxy, IPv6, TLS.",
						"thinkingSignature": "reasoning_content",
					},
					{ "type": "text", "text": "Let me check the TLS versions." },
					{ "type": "toolCall", "id": "call_1", "name": "bash", "arguments": { "command": "curl -v" } },
				]),
			),
			json!({
				"role": "toolResult",
				"toolCallId": "call_1",
				"toolName": "bash",
				"content": [{ "type": "text", "text": "TLSv1.2" }],
				"isError": false,
				"timestamp": NOW,
			}),
		]);

		assert_eq!(
			assistant_contents(&result)[0],
			json!([
				{ "type": "text", "text": "Let me check the TLS versions." },
				{ "type": "toolCall", "id": "call_1", "name": "bash", "arguments": { "command": "curl -v" } },
			])
		);
	}

	#[test]
	fn keeps_thinking_only_from_the_target_model_when_models_alternate() {
		let thinking = |text: &str| {
			json!({
				"type": "thinking",
				"thinking": text,
				"thinkingSignature": "reasoning_content",
			})
		};
		let stop = |mut message: Value| {
			message["stopReason"] = json!("stop");
			message
		};
		let result = transform(vec![
			json!({ "role": "user", "content": "1", "timestamp": NOW }),
			stop(assistant(
				"moonshotai/Kimi-K3",
				json!([thinking("kimi 1"), { "type": "text", "text": "a" }]),
			)),
			json!({ "role": "user", "content": "2", "timestamp": NOW }),
			stop(assistant(
				"zai-org/GLM-5.3",
				json!([thinking("glm 1"), { "type": "text", "text": "b" }]),
			)),
			json!({ "role": "user", "content": "3", "timestamp": NOW }),
			stop(assistant(
				"moonshotai/Kimi-K3",
				json!([thinking("kimi 2"), { "type": "text", "text": "c" }]),
			)),
			json!({ "role": "user", "content": "4", "timestamp": NOW }),
			stop(assistant(
				"zai-org/GLM-5.3",
				json!([thinking("glm 2"), { "type": "text", "text": "d" }]),
			)),
			json!({ "role": "user", "content": "hi", "timestamp": NOW }),
		]);

		assert_eq!(
			assistant_contents(&result),
			vec![
				json!([thinking("kimi 1"), { "type": "text", "text": "a" }]),
				json!([{ "type": "text", "text": "b" }]),
				json!([thinking("kimi 2"), { "type": "text", "text": "c" }]),
				json!([{ "type": "text", "text": "d" }]),
			]
		);
	}
}
