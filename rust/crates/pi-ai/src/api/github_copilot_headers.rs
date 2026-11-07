//! Port of `packages/ai/src/api/github-copilot-headers.ts`.
//! PORT-STATUS: pending

// PORT-STUB: w3-anthropic
// Implemented by w3-openai-completions, whose GitHub Copilot requests send these headers; the owning shard replaces
// this block with its port of the file.

use indexmap::IndexMap;

use crate::types::{Message, UserContent, UserMessageContent};

// Copilot expects X-Initiator to indicate whether the request is user-initiated
// or agent-initiated (e.g. follow-up after assistant/tool messages).
pub fn infer_copilot_initiator(messages: &[Message]) -> &'static str {
	match messages.last() {
		Some(last) if !matches!(last, Message::User(_)) => "agent",
		_ => "user",
	}
}

// Copilot requires Copilot-Vision-Request header when sending images
pub fn has_copilot_vision_input(messages: &[Message]) -> bool {
	messages.iter().any(|msg| match msg {
		Message::User(msg) => match &msg.content {
			UserMessageContent::Blocks(content) => content.iter().any(|c| matches!(c, UserContent::Image(_))),
			UserMessageContent::Text(_) => false,
		},
		Message::ToolResult(msg) => msg.content.iter().any(|c| matches!(c, UserContent::Image(_))),
		_ => false,
	})
}

/// `buildCopilotDynamicHeaders({ messages, hasImages })`.
pub fn build_copilot_dynamic_headers(messages: &[Message], has_images: bool) -> IndexMap<String, String> {
	let mut headers = IndexMap::from([
		("X-Initiator".to_string(), infer_copilot_initiator(messages).to_string()),
		("Openai-Intent".to_string(), "conversation-edits".to_string()),
	]);

	if has_images {
		headers.insert("Copilot-Vision-Request".into(), "true".into());
	}

	headers
}
