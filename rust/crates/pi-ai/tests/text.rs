//! Port of `packages/ai/test/text.test.ts`.

use pi_ai::types::{
	AssistantContent, ImageContent, SystemMessage, SystemMessageContent, TextContent, ThinkingContent, ToolCall,
	UserContent,
};
use pi_ai::utils::text::{content_text, get_system_message_text, render_system_message_update};

fn text(text: &str) -> TextContent {
	TextContent {
		text: text.into(),
		..TextContent::default()
	}
}

fn content() -> Vec<AssistantContent> {
	vec![
		AssistantContent::Thinking(ThinkingContent {
			thinking: "reasoning".into(),
			..ThinkingContent::default()
		}),
		AssistantContent::Text(text("first")),
		AssistantContent::ToolCall(ToolCall {
			id: "1".into(),
			name: "read".into(),
			..ToolCall::default()
		}),
		AssistantContent::Text(text("second")),
	]
}

#[test]
fn extracts_assistant_text_blocks() {
	assert_eq!(content_text(content().as_slice(), "\n"), "first\nsecond");
}

#[test]
fn supports_custom_separators() {
	assert_eq!(content_text(content().as_slice(), ""), "firstsecond");
}

#[test]
fn passes_string_content_through() {
	assert_eq!(content_text("hello", "\n"), "hello");
}

#[test]
fn extracts_text_from_tool_result_content() {
	let tool_result_content = vec![
		UserContent::Text(text("first")),
		UserContent::Image(ImageContent {
			data: "...".into(),
			mime_type: "image/png".into(),
			..ImageContent::default()
		}),
		UserContent::Text(text("second")),
	];
	assert_eq!(content_text(tool_result_content.as_slice(), ""), "firstsecond");
}

// Rust-only: section maps iterate in JS property order, so an integer-like name comes first as in `Object.entries`.
#[test]
fn renders_sections_in_js_property_order() {
	let message = SystemMessage {
		content: SystemMessageContent::Text("base".into()),
		sections: Some(
			[("b", Some("B")), ("1", Some("One")), ("gone", None)]
				.into_iter()
				.map(|(name, text)| (name.to_string(), text.map(String::from)))
				.collect(),
		),
		..SystemMessage::default()
	};
	assert_eq!(get_system_message_text(&message), "base\n\nOne\n\nB");
	assert_eq!(
		render_system_message_update(&message),
		"base\n\nUpdated system prompt section \"1\":\n\nOne\n\nUpdated system prompt section \"b\":\n\nB\n\nRemoved system prompt section \"gone\"."
	);
}
