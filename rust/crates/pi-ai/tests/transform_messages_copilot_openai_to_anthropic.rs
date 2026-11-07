//! Port of `packages/ai/test/transform-messages-copilot-openai-to-anthropic.test.ts`.
//!
//! Models and messages are built from the TS object literals as JSON. `Date.now()` timestamps are a fixed time.

use pi_ai::api::transform_messages::transform_messages;
use pi_ai::types::{AssistantContent, AssistantMessage, Message, Model};
use serde_json::{Value, json};

const NOW: f64 = 1_700_000_000_000.0;

// Normalize function matching what anthropic.ts uses
fn anthropic_normalize_tool_call_id(id: &str, _model: &Model, _source: &AssistantMessage) -> String {
	// `replace` works on UTF-16 code units, so an astral character becomes two underscores.
	id.encode_utf16()
		.map(|unit| match char::from_u32(u32::from(unit)) {
			Some(ch) if ch.is_ascii_alphanumeric() || ch == '_' || ch == '-' => ch,
			_ => '_',
		})
		.take(64)
		.collect()
}

fn make_copilot_claude_model() -> Model {
	serde_json::from_value(json!({
		"id": "claude-sonnet-4.6",
		"name": "Claude Sonnet 4.6",
		"api": "anthropic-messages",
		"provider": "github-copilot",
		"baseUrl": "https://api.individual.githubcopilot.com",
		"reasoning": true,
		"input": ["text", "image"],
		"cost": { "input": 0, "output": 0, "cacheRead": 0, "cacheWrite": 0 },
		"contextWindow": 128000,
		"maxTokens": 16000,
	}))
	.unwrap()
}

fn make_assistant_message(content: Value) -> Value {
	json!({
		"role": "assistant",
		"content": content,
		"api": "openai-responses",
		"provider": "github-copilot",
		"model": "gpt-5",
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

fn messages(values: Vec<Value>) -> Vec<Message> {
	serde_json::from_value(Value::Array(values)).unwrap()
}

/// `expect(actual).toMatchObject(expected)` for JSON objects.
fn assert_match_object(actual: &Value, expected: &Value) {
	match (actual, expected) {
		(Value::Object(actual), Value::Object(expected)) => {
			for (key, expected) in expected {
				assert_match_object(actual.get(key).unwrap_or(&Value::Null), expected);
			}
		}
		(Value::Array(actual), Value::Array(expected)) => {
			assert_eq!(actual.len(), expected.len());
			for (actual, expected) in actual.iter().zip(expected) {
				assert_match_object(actual, expected);
			}
		}
		_ => assert_eq!(actual, expected),
	}
}

mod open_ai_to_anthropic_session_migration_for_copilot_claude {
	use super::*;

	#[test]
	fn removes_thought_signature_from_tool_calls_when_migrating_between_models() {
		let model = make_copilot_claude_model();
		let messages = messages(vec![
			json!({ "role": "user", "content": "run a command", "timestamp": NOW }),
			json!({
				"role": "assistant",
				"content": [
					{
						"type": "toolCall",
						"id": "call_123",
						"name": "bash",
						"arguments": { "command": "ls" },
						"thoughtSignature": pi_host::js::json_stringify(&json!({ "type": "reasoning.encrypted", "id": "call_123", "data": "encrypted" })),
					},
				],
				"api": "openai-responses",
				"provider": "github-copilot",
				"model": "gpt-5",
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
			}),
			json!({
				"role": "toolResult",
				"toolCallId": "call_123",
				"toolName": "bash",
				"content": [{ "type": "text", "text": "output" }],
				"isError": false,
				"timestamp": NOW,
			}),
		]);

		let result = pi_host::testing::run(|_| async {
			transform_messages(&messages, &model, Some(&anthropic_normalize_tool_call_id))
		});
		let assistant_msg = result
			.iter()
			.find_map(|m| match m {
				Message::Assistant(assistant) => Some(assistant),
				_ => None,
			})
			.unwrap();
		let tool_call = assistant_msg
			.content
			.iter()
			.find_map(|b| match b {
				AssistantContent::ToolCall(tool_call) => Some(tool_call),
				_ => None,
			})
			.unwrap();

		assert_eq!(tool_call.thought_signature, None);
	}

	#[test]
	fn adds_synthetic_tool_results_for_trailing_orphaned_tool_calls() {
		let model = make_copilot_claude_model();
		let messages = messages(vec![
			json!({ "role": "user", "content": "read the file", "timestamp": NOW }),
			make_assistant_message(json!([
				{
					"type": "toolCall",
					"id": "call_123|fc_123",
					"name": "read",
					"arguments": { "path": "README.md" },
				},
			])),
		]);

		let result = pi_host::testing::run(|_| async {
			transform_messages(&messages, &model, Some(&anthropic_normalize_tool_call_id))
		});
		let last_message = serde_json::to_value(result.last().unwrap()).unwrap();

		assert_match_object(
			&last_message,
			&json!({
				"role": "toolResult",
				"toolCallId": "call_123_fc_123",
				"toolName": "read",
				"isError": true,
				"content": [{ "type": "text", "text": "No result provided" }],
			}),
		);
	}

	#[test]
	fn adds_synthetic_results_only_for_trailing_tool_calls_that_are_still_missing_results() {
		let model = make_copilot_claude_model();
		let messages = messages(vec![
			json!({ "role": "user", "content": "run commands", "timestamp": NOW }),
			make_assistant_message(json!([
				{ "type": "toolCall", "id": "call_1|fc_1", "name": "read", "arguments": { "path": "README.md" } },
				{ "type": "toolCall", "id": "call_2|fc_2", "name": "bash", "arguments": { "command": "pwd" } },
			])),
			json!({
				"role": "toolResult",
				"toolCallId": "call_1|fc_1",
				"toolName": "read",
				"content": [{ "type": "text", "text": "done" }],
				"isError": false,
				"timestamp": NOW,
			}),
		]);

		let result = pi_host::testing::run(|_| async {
			transform_messages(&messages, &model, Some(&anthropic_normalize_tool_call_id))
		});
		let synthetic_results: Vec<Value> = result
			.iter()
			.filter(|message| matches!(message, Message::ToolResult(tool_result) if tool_result.is_error))
			.map(|message| serde_json::to_value(message).unwrap())
			.collect();

		assert_eq!(synthetic_results.len(), 1);
		assert_match_object(
			&synthetic_results[0],
			&json!({
				"role": "toolResult",
				"toolCallId": "call_2_fc_2",
				"toolName": "bash",
				"content": [{ "type": "text", "text": "No result provided" }],
			}),
		);
	}
}
