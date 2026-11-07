//! Port of `packages/ai/test/openai-responses-foreign-toolcall-id.test.ts`.

mod openai_responses_support;

use openai_responses_support::{context, usage};
use pi_ai::api::openai_responses_shared::convert_responses_messages;
use pi_ai::providers::all::get_builtin_model;
use pi_ai::utils::hash::short_hash;
use regex::Regex;
use serde_json::json;

const COPILOT_RAW_TOOL_CALL_ID: &str = "call_4VnzVawQXPB9MgYib7CiQFEY|I9b95oN1wD/cHXKTw3PpRkL6KkCtzTJhUxMouMWYwHeTo2j3htzfSk7YPx2vifiIM4g3A8XXyOj8q4Bt6SLUG7gqY1E3ELkrkVQNHglRfUmWj84lqxJY+Puieb3VKyX0FB+83TUzn91cDMF/4gzt990IzqVrc+nIb9RRscRD070Du16q1glydVjWR0SBJsE6TbY/esOjFpqplogQqrajm1eI++f3eLi73R6q7hVusY0QbeFySVxABCjhN0lXB04caBe1rzHjYzul6MAXj7uq+0r17VLq+yrtyYhN12wkmFqHeqTyEei6EFPbMy24Nc+IbJlkP0OCg02W+gOnyBFcbi2ctvJFSOhSjt1CqBdqCnnhwUqXjbWiT0wh3DmLScRgTHmGkaI+oAcQQjfic65nxj+TnEkReA==";

mod open_ai_responses_foreign_tool_call_id_normalization {
	use super::*;

	#[test]
	fn hashes_foreign_copilot_tool_item_ids_into_a_bounded_codex_safe_fc_hash_shape() {
		let model = get_builtin_model("openai-codex", "gpt-6.1-sol").expect("model is defined");
		let now = 1_700_000_000_000.0;
		let context = context(json!({
			"systemPrompt": "You are concise.",
			"messages": [
				{ "role": "user", "content": "Use the tool.", "timestamp": now - 3000.0 },
				{
					"role": "assistant",
					"content": [{
						"type": "toolCall",
						"id": COPILOT_RAW_TOOL_CALL_ID,
						"name": "edit",
						"arguments": { "path": "src/styles/app.css" },
					}],
					"api": "openai-responses",
					"provider": "github-copilot",
					"model": "gpt-5.5",
					"usage": usage(),
					"stopReason": "toolUse",
					"timestamp": now - 2000.0,
				},
				{
					"role": "toolResult",
					"toolCallId": COPILOT_RAW_TOOL_CALL_ID,
					"toolName": "edit",
					"content": [{ "type": "text", "text": "ok" }],
					"isError": false,
					"timestamp": now - 1000.0,
				},
			],
		}));

		let input =
			convert_responses_messages(&model, &context, &["openai", "openai-codex", "opencode"], None).unwrap();
		let function_call = input
			.iter()
			.find(|item| item["type"] == "function_call")
			.expect("Expected function_call item");

		let expected_item_id = format!("fc_{}", short_hash(COPILOT_RAW_TOOL_CALL_ID.split('|').nth(1).unwrap()));
		let id = function_call["id"].as_str().expect("id is a string");
		assert_eq!(id, expected_item_id);
		assert!(pi_host::js::utf16_len(id) <= 64);
		assert!(Regex::new("^fc_[A-Za-z0-9]+$").unwrap().is_match(id));
	}
}
