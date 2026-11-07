//! Port of `packages/ai/test/openai-responses-message-id.test.ts`.

mod openai_responses_support;

use openai_responses_support::{context, usage};
use pi_ai::api::openai_responses_shared::convert_responses_messages;
use pi_ai::providers::all::get_builtin_model;
use serde_json::json;

mod open_ai_responses_message_id_conversion {
	use super::*;

	#[test]
	fn generates_unique_fallback_message_ids_for_multiple_text_blocks_in_one_assistant_turn() {
		let model = get_builtin_model("openai-codex", "gpt-6.1-sol").expect("model is defined");
		let now = 1_700_000_000_000.0;
		let context = context(json!({
			"systemPrompt": "You are concise.",
			"messages": [
				{ "role": "user", "content": "hello", "timestamp": now - 2000.0 },
				{
					"role": "assistant",
					"content": [
						{ "type": "text", "text": "first answer" },
						{ "type": "text", "text": "visible answer" },
					],
					"api": "anthropic-messages",
					"provider": "anthropic",
					"model": "claude-opus-4-8",
					"usage": usage(),
					"stopReason": "stop",
					"timestamp": now - 1000.0,
				},
			],
		}));

		let input =
			convert_responses_messages(&model, &context, &["openai", "openai-codex", "opencode"], None).unwrap();
		let message_ids: Vec<&str> = input
			.iter()
			.filter(|item| item["type"] == "message")
			.filter_map(|item| item.get("id").and_then(|id| id.as_str()))
			.collect();

		assert_eq!(message_ids, ["msg_pi_1", "msg_pi_1_1"]);
		let unique: std::collections::HashSet<&&str> = message_ids.iter().collect();
		assert_eq!(unique.len(), message_ids.len());
	}
}
