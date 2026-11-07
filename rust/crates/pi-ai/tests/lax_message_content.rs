//! Port of `packages/ai/test/lax-message-content.test.ts`.
//!
//! The Message types require `content` to always be present, but untyped
//! callers (custom tools, hand-built histories, old session files) can violate
//! that contract. `transformMessages` is the choke point before every provider
//! request and is intentionally lax: it normalizes null/missing content to an
//! empty array (issues #6259, #6276).
//!
//! Rust: the message types read a `null` or missing `content` as an empty list when they deserialize, so the
//! messages below reach `transform_messages` already normalized.

use pi_ai::api::transform_messages::transform_messages;
use pi_ai::types::{Message, Model, UserMessageContent};
use serde_json::json;

const NOW: f64 = 1_700_000_000_000.0;

// Text-only model so the image downgrade path (replaceImagesWithPlaceholder) runs,
// which was the primary crash site for null tool result content.
fn make_text_only_model() -> Model {
	serde_json::from_value(json!({
		"id": "test-model",
		"name": "Test Model",
		"api": "openai-completions",
		"provider": "openai",
		"baseUrl": "https://example.invalid/v1",
		"reasoning": false,
		"input": ["text"],
		"cost": { "input": 0, "output": 0, "cacheRead": 0, "cacheWrite": 0 },
		"contextWindow": 128000,
		"maxTokens": 16000,
	}))
	.unwrap()
}

mod lax_message_content_handling {
	use super::*;

	#[test]
	fn normalizes_null_missing_content_to_an_empty_array_instead_of_crashing() {
		let messages: Vec<Message> = serde_json::from_value(json!([
			{ "role": "user", "content": null, "timestamp": NOW },
			{
				"role": "assistant",
				"content": null,
				"api": "openai-completions",
				"provider": "openai",
				"model": "test-model",
				"usage": {
					"input": 0,
					"output": 0,
					"cacheRead": 0,
					"cacheWrite": 0,
					"totalTokens": 0,
					"cost": { "input": 0, "output": 0, "cacheRead": 0, "cacheWrite": 0, "total": 0 },
				},
				"stopReason": "stop",
				"timestamp": NOW,
			},
			{
				"role": "toolResult",
				"toolCallId": "call_1",
				"toolName": "web_search",
				"isError": false,
				"timestamp": NOW,
			},
		]))
		.unwrap();

		let result = pi_host::testing::run(|_| async { transform_messages(&messages, &make_text_only_model(), None) });

		assert_eq!(result.len(), 3);
		for msg in &result {
			let empty = match msg {
				Message::User(user) => matches!(&user.content, UserMessageContent::Blocks(blocks) if blocks.is_empty()),
				Message::Assistant(assistant) => assistant.content.is_empty(),
				Message::ToolResult(tool_result) => tool_result.content.is_empty(),
				Message::System(_) => false,
			};
			assert!(empty, "{msg:?}");
		}
	}
}
