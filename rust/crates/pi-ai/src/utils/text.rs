//! Port of `packages/ai/src/utils/text.ts`.
//! PORT-STATUS: ported
//!
//! `contentText` takes a string or any content block array in TS; in Rust it takes anything that implements
//! [`ContentText`]. `js_trim` is Rust-only: `String.prototype.trim`, which other ports in this crate need.

use crate::types::{
	AssistantContent, SystemMessage, SystemMessageContent, TextContent, UserContent, UserMessageContent,
};

/// A content block that may hold text: `TextContent | ImageContent | ThinkingContent | ToolCall`.
pub trait TextBlock {
	/// The text of a `text` block; `None` for every other block.
	fn block_text(&self) -> Option<&str>;
}

impl TextBlock for TextContent {
	fn block_text(&self) -> Option<&str> {
		Some(&self.text)
	}
}

impl TextBlock for UserContent {
	fn block_text(&self) -> Option<&str> {
		match self {
			UserContent::Text(block) => Some(&block.text),
			UserContent::Image(_) => None,
		}
	}
}

impl TextBlock for AssistantContent {
	fn block_text(&self) -> Option<&str> {
		match self {
			AssistantContent::Text(block) => Some(&block.text),
			_ => None,
		}
	}
}

/// Message content: `string | readonly Content[]`.
pub trait ContentText {
	/// Extract and join text from message content.
	fn content_text(&self, separator: &str) -> String;
}

impl ContentText for str {
	fn content_text(&self, _separator: &str) -> String {
		self.to_string()
	}
}

impl<B: TextBlock> ContentText for [B] {
	fn content_text(&self, separator: &str) -> String {
		self.iter()
			.filter_map(TextBlock::block_text)
			.collect::<Vec<_>>()
			.join(separator)
	}
}

impl ContentText for UserMessageContent {
	fn content_text(&self, separator: &str) -> String {
		match self {
			UserMessageContent::Text(text) => text.clone(),
			UserMessageContent::Blocks(blocks) => blocks.content_text(separator),
		}
	}
}

impl ContentText for SystemMessageContent {
	fn content_text(&self, separator: &str) -> String {
		match self {
			SystemMessageContent::Text(text) => text.clone(),
			SystemMessageContent::Blocks(blocks) => blocks.content_text(separator),
		}
	}
}

/// Extract and join text from message content. TS's default separator is `"\n"`.
pub fn content_text<C: ContentText + ?Sized>(content: &C, separator: &str) -> String {
	content.content_text(separator)
}

/// Render a system message as a complete prompt: its content followed by its sections.
pub fn get_system_message_text(message: &SystemMessage) -> String {
	let mut parts = vec![message.content.content_text("\n")];
	for text in message.sections.iter().flat_map(|sections| sections.values()).flatten() {
		parts.push(text.clone());
	}
	parts.retain(|part| !part.is_empty());
	parts.join("\n\n")
}

/// Render a later system message for APIs that accept system messages mid-conversation.
/// Section changes are framed by name so the model can relate them to the leading prompt.
/// This framing is request-time only and may change between versions.
pub fn render_system_message_update(message: &SystemMessage) -> String {
	let mut parts = Vec::new();
	let text = message.content.content_text("\n");
	if !text.is_empty() {
		parts.push(text);
	}
	for (name, value) in message.sections.iter().flatten() {
		parts.push(match value {
			None => format!("Removed system prompt section \"{name}\"."),
			Some(value) => format!("Updated system prompt section \"{name}\":\n\n{value}"),
		});
	}
	parts.join("\n\n")
}

/// `text.slice(start, end)` in UTF-16 code units, as a string of its own. Rust strings never share their buffer, so
/// this is the plain slice the TS works around V8 to get. A cut inside a surrogate pair drops the lone half.
pub fn owned_slice(text: &str, start: usize, end: Option<usize>) -> String {
	let end = end.unwrap_or(usize::MAX);
	let mut units = 0;
	let mut result = String::new();
	for ch in text.chars() {
		let width = ch.len_utf16();
		if units >= start && units + width <= end {
			result.push(ch);
		}
		units += width;
		if units >= end {
			break;
		}
	}
	result
}

/// `text.trim()`: removes JavaScript white space and line terminators, which include U+FEFF but not U+0085.
pub fn js_trim(text: &str) -> &str {
	text.trim_matches(|ch: char| (ch.is_whitespace() && ch != '\u{85}') || ch == '\u{feff}')
}
