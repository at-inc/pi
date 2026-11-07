//! Port of `packages/ai/src/api/github-copilot-headers.ts`.
//! PORT-STATUS: pending

// PORT-STUB: w3-anthropic
// Predicted by w3-openai-responses for `openai_responses.rs`. `buildCopilotDynamicHeaders({ messages, hasImages })`
// takes the two fields as arguments and returns the headers in insertion order.
use indexmap::IndexMap;

use crate::types::Message;

pub fn has_copilot_vision_input(_messages: &[Message]) -> bool {
	todo!("PORT-STUB")
}

pub fn build_copilot_dynamic_headers(_messages: &[Message], _has_images: bool) -> IndexMap<String, String> {
	todo!("PORT-STUB")
}
