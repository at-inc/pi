//! Port of `packages/ai/test/system-message-replay.test.ts`.
//!
//! Messages are written as JSON and deserialized, and `toEqual` compares serialized JSON. The executable tool in
//! "compares tool declarations" carries its extra field in `Tool.extra`; Rust tools have no `execute` function.

#![allow(deprecated, reason = "the TS test exercises the deprecated hasToolRedefinitions too")]

use pi_ai::types::{Context, Message, Tool, ToolConstrainedSampling, TranscriptContext};
use pi_ai::utils::text::{get_system_message_text, render_system_message_update};
use pi_ai::utils::transcript::{
	collapse_system_messages, declarations_equal, get_current_system_message, get_current_system_prompt,
	get_tool_state_changes, has_non_additive_tool_changes, has_tool_redefinitions, normalize_context,
};
use serde_json::{Value, json};

fn tool(name: &str) -> Tool {
	tool_with_description(name, &format!("{name} tool"))
}

fn tool_with_description(name: &str, description: &str) -> Tool {
	Tool {
		name: name.into(),
		description: description.into(),
		parameters: json!({ "type": "object", "properties": {} }),
		..Tool::default()
	}
}

fn tool_json(name: &str) -> Value {
	serde_json::to_value(tool(name)).unwrap()
}

fn messages(value: Value) -> Vec<Message> {
	serde_json::from_value(value).unwrap()
}

fn normalize(messages_json: Value) -> TranscriptContext {
	normalize_context(&Context {
		messages: messages(messages_json),
		..Context::default()
	})
}

fn transcript() -> TranscriptContext {
	normalize(json!([
		{
			"role": "system",
			"content": "base",
			"sections": { "a": "<a>1</a>", "b": "<b>1</b>" },
			"toolsAdded": [tool_json("first")],
			"timestamp": 10,
		},
		{ "role": "user", "content": "hello", "timestamp": 11 },
		{ "role": "system", "content": "also do this", "timestamp": 12 },
		{
			"role": "assistant",
			"content": [{ "type": "text", "text": "ok" }],
			"api": "openai-responses",
			"provider": "openai",
			"model": "test",
			"usage": {
				"input": 0, "output": 0, "cacheRead": 0, "cacheWrite": 0, "totalTokens": 0,
				"cost": { "input": 0, "output": 0, "cacheRead": 0, "cacheWrite": 0, "total": 0 },
			},
			"stopReason": "stop",
			"timestamp": 13,
		},
		{
			"role": "system",
			"content": "",
			"sections": { "a": "<a>2</a>", "b": null, "c": "<c>1</c>" },
			"toolsRemoved": [{ "name": "first" }],
			"toolsAdded": [tool_json("second")],
			"timestamp": 14,
		},
	]))
}

fn roles(context: &TranscriptContext) -> Vec<&'static str> {
	context
		.messages
		.iter()
		.map(|message| match message {
			Message::System(_) => "system",
			Message::User(_) => "user",
			Message::Assistant(_) => "assistant",
			Message::ToolResult(_) => "toolResult",
		})
		.collect()
}

mod system_message_replay {
	use super::*;

	#[test]
	fn replays_content_sections_and_tools_into_one_leading_message() {
		let transcript = transcript();
		let current = get_current_system_message(&transcript.messages);
		assert_eq!(
			serde_json::to_value(current).unwrap(),
			json!({
				"role": "system",
				"content": "base\n\nalso do this",
				"sections": { "a": "<a>2</a>", "c": "<c>1</c>" },
				"toolsAdded": [tool_json("second")],
				"timestamp": 10.0,
			})
		);
		assert_eq!(
			get_current_system_prompt(&transcript.messages),
			"base\n\nalso do this\n\n<a>2</a>\n\n<c>1</c>"
		);
	}

	#[test]
	fn collapse_keeps_only_non_system_messages_after_the_replayed_head() {
		let collapsed = collapse_system_messages(&transcript());
		assert_eq!(roles(&collapsed), ["system", "user", "assistant"]);
		assert_eq!(collapse_system_messages(&collapsed), collapsed);
	}

	#[test]
	fn replay_of_a_transcript_without_system_messages_is_empty() {
		let context = normalize(json!([{ "role": "user", "content": "hi", "timestamp": 1 }]));
		assert_eq!(get_current_system_message(&context.messages), None);
		assert_eq!(get_current_system_prompt(&context.messages), "");
		assert_eq!(collapse_system_messages(&context).messages, context.messages);
	}

	#[test]
	fn a_late_full_patch_on_a_transcript_without_a_leading_message_replays_as_the_prompt() {
		let context = normalize(json!([
			{ "role": "user", "content": "old session", "timestamp": 1 },
			{
				"role": "system",
				"content": "",
				"sections": { "preamble": "You are pi." },
				"toolsAdded": [tool_json("x")],
				"timestamp": 2,
			},
		]));
		assert_eq!(get_current_system_prompt(&context.messages), "You are pi.");
		let Message::System(first) = &collapse_system_messages(&context).messages[0] else {
			panic!("expected a system message");
		};
		assert_eq!(first.tools_added, Some(vec![tool("x")]));
	}

	#[test]
	fn renders_complete_prompts_and_framed_updates() {
		let transcript = transcript();
		let (Message::System(leading), Message::System(update)) = (&transcript.messages[0], &transcript.messages[4])
		else {
			panic!("expected system messages");
		};
		assert_eq!(get_system_message_text(leading), "base\n\n<a>1</a>\n\n<b>1</b>");
		assert_eq!(
			render_system_message_update(update),
			[
				"Updated system prompt section \"a\":\n\n<a>2</a>",
				"Removed system prompt section \"b\".",
				"Updated system prompt section \"c\":\n\n<c>1</c>",
			]
			.join("\n\n")
		);
	}

	#[test]
	fn normalizes_the_legacy_prompt_and_tool_fields_into_a_leading_system_message() {
		let user = json!([{ "role": "user", "content": "hi", "timestamp": 1 }]);
		assert_eq!(
			normalize_context(&Context {
				messages: messages(user.clone()),
				..Context::default()
			})
			.messages,
			messages(user.clone())
		);
		assert_eq!(
			normalize_context(&Context {
				system_prompt: Some(String::new()),
				tools: Some(vec![]),
				messages: messages(user.clone()),
			})
			.messages,
			messages(user.clone())
		);
		let normalized = normalize_context(&Context {
			system_prompt: Some("be brief".into()),
			tools: Some(vec![tool("a")]),
			messages: messages(user.clone()),
		});
		assert_eq!(
			serde_json::to_value(&normalized.messages).unwrap(),
			json!([
				{ "role": "system", "content": "be brief", "toolsAdded": [tool_json("a")], "timestamp": 0.0 },
				{ "role": "user", "content": "hi", "timestamp": 1.0 },
			])
		);
	}

	#[test]
	fn compares_tool_declarations_without_executable_or_undefined_fields() {
		let mut executable = tool("a");
		executable.extra.insert("execute".into(), json!({}));
		assert!(declarations_equal(&executable, &tool("a")));
		assert!(!declarations_equal(&tool("a"), &tool_with_description("a", "changed")));
		let disabled = Tool {
			constrained_sampling: Some(ToolConstrainedSampling::Disabled(false)),
			..tool("a")
		};
		assert!(!declarations_equal(&tool("a"), &disabled));
	}

	#[test]
	fn tool_state_changes_treat_changed_definitions_as_removal_plus_addition() {
		let changes = get_tool_state_changes(
			&[tool("a"), tool("b")],
			&[tool_with_description("b", "changed"), tool("c")],
		);
		assert_eq!(
			changes.tools_added,
			vec![tool_with_description("b", "changed"), tool("c")]
		);
		assert_eq!(
			changes
				.tools_removed
				.iter()
				.map(|reference| reference.name.as_str())
				.collect::<Vec<_>>(),
			["a", "b"]
		);
		let unchanged = get_tool_state_changes(&[tool("a")], &[tool("a")]);
		assert!(unchanged.tools_added.is_empty());
		assert!(unchanged.tools_removed.is_empty());
	}

	#[test]
	fn detects_non_additive_tool_history_and_redefinitions() {
		let transcript = transcript();
		assert!(has_non_additive_tool_changes(&transcript.messages));
		assert!(!has_tool_redefinitions(&transcript.messages));
		let additive = normalize(json!([
			{ "role": "system", "content": "", "toolsAdded": [tool_json("a")], "timestamp": 1 },
			{ "role": "system", "content": "", "toolsAdded": [tool_json("b")], "timestamp": 2 },
		]));
		assert!(!has_non_additive_tool_changes(&additive.messages));
		let mut changed = tool_json("a");
		changed["description"] = json!("changed");
		let redeclared = normalize(json!([
			{ "role": "system", "content": "", "toolsAdded": [tool_json("a")], "timestamp": 1 },
			{ "role": "system", "content": "", "toolsAdded": [changed], "timestamp": 2 },
		]));
		assert!(has_non_additive_tool_changes(&redeclared.messages));
		assert!(has_tool_redefinitions(&redeclared.messages));
	}
}
