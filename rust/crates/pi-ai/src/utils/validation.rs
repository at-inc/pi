//! Port of `packages/ai/src/utils/validation.ts`.
//! PORT-STATUS: pending

use pi_host::PiResult;
use serde_json::Value;

use crate::types::{Tool, ToolCall};

// PORT-STUB: w2-ai-core
/// Validates tool call arguments against the tool's JSON Schema, coercing them first.
pub fn validate_tool_arguments(_tool: &Tool, _tool_call: &ToolCall) -> PiResult<Value> {
	todo!("PORT-STUB")
}
