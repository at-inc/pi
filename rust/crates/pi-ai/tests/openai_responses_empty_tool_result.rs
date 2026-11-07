//! Port of `packages/ai/test/openai-responses-empty-tool-result.test.ts`.

mod openai_responses_support;

use openai_responses_support::{context, usage};
use pi_ai::api::openai_responses_shared::convert_responses_messages;
use pi_ai::providers::all::get_builtin_model;
use serde_json::{Value, json};

fn build_empty_tool_result(tool_call_id: &str, timestamp: f64, text: &str) -> Value {
	json!({
		"role": "toolResult",
		"toolCallId": tool_call_id,
		"toolName": "bash",
		"content": [{ "type": "text", "text": text }],
		"isError": false,
		"timestamp": timestamp,
	})
}

mod open_ai_responses_convert_responses_messages_empty_tool_result {
	use super::*;

	// Whitespace-only cases: https://github.com/earendil-works/pi/issues/8720
	#[test]
	fn uses_no_tool_output_placeholder_for_tool_result_text_without_images() {
		for text in ["", "\r\n", " \n\t"] {
			let model = get_builtin_model("openai", "gpt-4o-mini").expect("model is defined");
			let now = 1_700_000_000_000.0;
			let context = context(json!({
				"messages": [
					{ "role": "user", "content": "Run the command", "timestamp": now - 1.0 },
					{
						"role": "assistant",
						"content": [{ "type": "toolCall", "id": "tool-1", "name": "bash", "arguments": { "command": "true" } }],
						"api": model.api,
						"provider": model.provider,
						"model": model.id,
						"usage": usage(),
						"stopReason": "toolUse",
						"timestamp": now,
					},
					build_empty_tool_result("tool-1", now + 1.0, text),
				],
			}));

			let input =
				convert_responses_messages(&model, &context, &["openai", "openai-codex", "opencode"], None).unwrap();
			let function_call_output = input
				.iter()
				.find(|item| item["type"] == "function_call_output")
				.unwrap_or_else(|| panic!("function_call_output for {text:?}"));

			let output = function_call_output["output"].as_str().expect("output is a string");
			assert_eq!(output, "(no tool output)", "{text:?}");
			assert!(!output.contains("see attached image"));
		}
	}
}
