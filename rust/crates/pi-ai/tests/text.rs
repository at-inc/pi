//! Port of `packages/ai/test/text.test.ts`.

use pi_ai::types::{AssistantContent, ImageContent, TextContent, ThinkingContent, ToolCall, UserContent};
use pi_ai::utils::text::content_text;

fn text(text: &str) -> TextContent {
	TextContent {
		text: text.into(),
		text_signature: None,
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
		}),
		UserContent::Text(text("second")),
	];
	assert_eq!(content_text(tool_result_content.as_slice(), ""), "firstsecond");
}
