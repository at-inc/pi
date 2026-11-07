//! Port of `packages/ai/src/utils/estimate.ts`.
//! PORT-STATUS: ported
//!
//! `estimateContextTokens` takes `TranscriptContext | readonly Message[]` in TS; the Rust port takes the messages
//! (pass `&context.messages` for a `TranscriptContext`). `string | Array<TextContent | ImageContent>` parameters take
//! anything that implements [`TextAndImageContent`]. `safeJsonStringify` cannot fail in Rust (every value serializes),
//! so its `"undefined"` and `"[unserializable]"` fallbacks are unreachable and dropped.

use pi_host::js::{json_stringify, utf16_len};
use serde::Serialize;

use super::text::get_system_message_text;
use crate::types::{AssistantContent, Message, StopReason, Usage, UserContent, UserMessageContent};

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ContextUsageEstimate {
	/// Estimated total context tokens.
	pub tokens: f64,
	/// Tokens reported by the most recent applicable assistant usage block.
	pub usage_tokens: f64,
	/// Estimated tokens after the most recent applicable assistant usage block.
	pub trailing_tokens: f64,
	/// Index of the applicable message that provided usage, or null when none exists.
	pub last_usage_index: Option<usize>,
}

const CHARS_PER_TOKEN: f64 = 3.5;
const ESTIMATED_IMAGE_CHARS: f64 = 4800.0;

pub fn calculate_context_tokens(usage: &Usage) -> f64 {
	if usage.total_tokens != 0.0 && !usage.total_tokens.is_nan() {
		return usage.total_tokens;
	}
	usage.input + usage.output + usage.cache_read + usage.cache_write
}

/// `string | Array<TextContent | ImageContent>`: content whose characters `estimateTextAndImageContentTokens` counts.
pub trait TextAndImageContent {
	/// `estimateTextAndImageContentChars(content)`.
	fn estimate_chars(&self) -> f64;
}

impl TextAndImageContent for str {
	fn estimate_chars(&self) -> f64 {
		utf16_len(self) as f64
	}
}

impl TextAndImageContent for [UserContent] {
	fn estimate_chars(&self) -> f64 {
		let mut chars = 0.0;
		for block in self {
			chars += match block {
				UserContent::Text(block) => utf16_len(&block.text) as f64,
				UserContent::Image(_) => ESTIMATED_IMAGE_CHARS,
			};
		}
		chars
	}
}

impl TextAndImageContent for Vec<UserContent> {
	fn estimate_chars(&self) -> f64 {
		self.as_slice().estimate_chars()
	}
}

impl TextAndImageContent for UserMessageContent {
	fn estimate_chars(&self) -> f64 {
		match self {
			UserMessageContent::Text(text) => text.estimate_chars(),
			UserMessageContent::Blocks(blocks) => blocks.estimate_chars(),
		}
	}
}

pub fn estimate_text_tokens(text: &str) -> f64 {
	(utf16_len(text) as f64 / CHARS_PER_TOKEN).ceil()
}

pub fn estimate_text_and_image_content_tokens<C: TextAndImageContent + ?Sized>(content: &C) -> f64 {
	(content.estimate_chars() / CHARS_PER_TOKEN).ceil()
}

pub fn estimate_message_tokens(message: &Message) -> f64 {
	let mut chars = 0.0;

	let assistant = match message {
		Message::System(message) => {
			return estimate_text_tokens(&get_system_message_text(message))
				+ estimate_tools_tokens(message.tools_added.as_deref())
				+ estimate_tools_tokens(message.tools_removed.as_deref());
		}
		Message::User(message) => return estimate_text_and_image_content_tokens(&message.content),
		Message::ToolResult(message) => return estimate_text_and_image_content_tokens(&message.content),
		Message::Assistant(message) => message,
	};

	for block in &assistant.content {
		match block {
			AssistantContent::Text(block) => chars += utf16_len(&block.text) as f64,
			AssistantContent::Thinking(block) => chars += utf16_len(&block.thinking) as f64,
			AssistantContent::ToolCall(block) => {
				chars += (utf16_len(&block.name) + utf16_len(&json_stringify(&block.arguments))) as f64;
			}
		}
	}
	(chars / CHARS_PER_TOKEN).ceil()
}

fn get_last_assistant_usage_info(messages: &[Message]) -> Option<(&Usage, usize)> {
	let mut latest_prefix_timestamp = f64::NEG_INFINITY;
	let mut usage_info = None;

	for (i, message) in messages.iter().enumerate() {
		if let Message::Assistant(assistant) = message {
			// A newer prefix message was inserted after this response (for example, a
			// compaction summary), so its usage cannot describe the current prefix.
			let usage_applies_to_prefix = assistant.timestamp >= latest_prefix_timestamp;
			if usage_applies_to_prefix
				&& assistant.stop_reason != StopReason::Aborted
				&& assistant.stop_reason != StopReason::Error
				&& calculate_context_tokens(&assistant.usage) > 0.0
			{
				usage_info = Some((&assistant.usage, i));
			}
		}
		let timestamp = message_timestamp(message);
		// `Math.max`: NaN wins, where `f64::max` would drop it.
		latest_prefix_timestamp = if latest_prefix_timestamp.is_nan() || timestamp.is_nan() {
			f64::NAN
		} else {
			latest_prefix_timestamp.max(timestamp)
		};
	}

	usage_info
}

fn message_timestamp(message: &Message) -> f64 {
	match message {
		Message::System(message) => message.timestamp,
		Message::User(message) => message.timestamp,
		Message::Assistant(message) => message.timestamp,
		Message::ToolResult(message) => message.timestamp,
	}
}

pub fn estimate_context_tokens(messages: &[Message]) -> ContextUsageEstimate {
	if let Some((usage, index)) = get_last_assistant_usage_info(messages) {
		let usage_tokens = calculate_context_tokens(usage);
		let mut trailing_tokens = 0.0;
		for message in &messages[index + 1..] {
			trailing_tokens += estimate_message_tokens(message);
		}
		return ContextUsageEstimate {
			tokens: usage_tokens + trailing_tokens,
			usage_tokens,
			trailing_tokens,
			last_usage_index: Some(index),
		};
	}

	let mut tokens = 0.0;
	for message in messages {
		tokens += estimate_message_tokens(message);
	}
	ContextUsageEstimate {
		tokens,
		usage_tokens: 0.0,
		trailing_tokens: tokens,
		last_usage_index: None,
	}
}

fn estimate_tools_tokens<T: Serialize>(tools: Option<&[T]>) -> f64 {
	let Some(tools) = tools.filter(|tools| !tools.is_empty()) else {
		return 0.0;
	};
	estimate_text_tokens(&json_stringify(tools))
}
