//! Port of `packages/ai/src/api/github-copilot-headers.ts`.
//! PORT-STATUS: ported
//!
//! `buildCopilotDynamicHeaders`' `params` object becomes two arguments.

use indexmap::IndexMap;
use serde::{Deserialize, Serialize};

use crate::types::{Message, UserContent, UserMessageContent};

/// `"user" | "agent"`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum CopilotInitiator {
	#[serde(rename = "user")]
	User,
	#[serde(rename = "agent")]
	Agent,
}

impl CopilotInitiator {
	/// The header value.
	pub fn as_str(self) -> &'static str {
		match self {
			CopilotInitiator::User => "user",
			CopilotInitiator::Agent => "agent",
		}
	}
}

// Copilot expects X-Initiator to indicate whether the request is user-initiated
// or agent-initiated (e.g. follow-up after assistant/tool messages).
pub fn infer_copilot_initiator(messages: &[Message]) -> CopilotInitiator {
	match messages.last() {
		Some(last) if !matches!(last, Message::User(_)) => CopilotInitiator::Agent,
		_ => CopilotInitiator::User,
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

pub fn build_copilot_dynamic_headers(messages: &[Message], has_images: bool) -> IndexMap<String, String> {
	let mut headers = IndexMap::from([
		(
			"X-Initiator".to_string(),
			infer_copilot_initiator(messages).as_str().to_string(),
		),
		("Openai-Intent".to_string(), "conversation-edits".to_string()),
	]);

	if has_images {
		headers.insert("Copilot-Vision-Request".to_string(), "true".to_string());
	}

	headers
}
