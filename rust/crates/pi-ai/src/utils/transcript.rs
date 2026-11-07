//! Port of `packages/ai/src/utils/transcript.ts`.
//! PORT-STATUS: pending

use crate::types::{Context, Message, SystemMessage, Tool, ToolReference};

pub use crate::types::TranscriptContext;

// PORT-STUB: w2-ai-core
/// Build the leading system message for a prompt and tool set. Returns undefined when
/// both are empty, so an empty transcript stays empty.
pub fn create_initial_system_message(_system_prompt: Option<&str>, _tools: Option<&[Tool]>) -> Option<SystemMessage> {
	todo!("PORT-STUB")
}

// PORT-STUB: w2-ai-core
/// Fold `Context.systemPrompt` and `Context.tools` into a leading system message.
pub fn normalize_context(_context: &Context) -> TranscriptContext {
	todo!("PORT-STUB")
}

// PORT-STUB: w2-ai-core
/// Any message list. The replay helpers only read entries whose role is `"system"`, so
/// agent transcripts that carry custom message roles can be passed without filtering.
pub trait TranscriptMessage {
	/// The message when its role is `"system"`.
	fn as_system_message(&self) -> Option<&SystemMessage>;
}

// PORT-STUB: w2-ai-core
impl TranscriptMessage for Message {
	fn as_system_message(&self) -> Option<&SystemMessage> {
		match self {
			Message::System(message) => Some(message),
			_ => None,
		}
	}
}

// PORT-STUB: w2-ai-core
impl TranscriptMessage for SystemMessage {
	fn as_system_message(&self) -> Option<&SystemMessage> {
		Some(self)
	}
}

// PORT-STUB: w2-ai-core
/// Resolve the tools available after applying every transcript delta in order.
pub fn get_current_tools<M: TranscriptMessage>(_messages: &[M]) -> Vec<Tool> {
	todo!("PORT-STUB")
}

// PORT-STUB: w2-ai-core
/// Replay every system message into one leading system message holding the current prompt and tools.
pub fn get_current_system_message<M: TranscriptMessage>(_messages: &[M]) -> Option<SystemMessage> {
	todo!("PORT-STUB")
}

// PORT-STUB: w2-ai-core
/// Render the current system prompt text after replaying every system message.
pub fn get_current_system_prompt<M: TranscriptMessage>(_messages: &[M]) -> String {
	todo!("PORT-STUB")
}

// PORT-STUB: w2-ai-core
/// Strip executable and display-only fields from a tool before transcript comparison or persistence.
pub fn to_tool_declaration(_tool: &Tool) -> Tool {
	todo!("PORT-STUB")
}

// PORT-STUB: w2-ai-core
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ToolStateChanges {
	pub tools_added: Vec<Tool>,
	pub tools_removed: Vec<ToolReference>,
}

// PORT-STUB: w2-ai-core
/// Compare two complete tool states. A changed definition is a removal followed by an addition.
pub fn get_tool_state_changes(_previous: &[Tool], _current: &[Tool]) -> ToolStateChanges {
	todo!("PORT-STUB")
}
