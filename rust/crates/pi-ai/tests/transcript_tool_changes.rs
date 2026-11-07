//! Port of `packages/ai/test/transcript-tool-changes.test.ts`.
//!
//! Only "resolves the top-level tool boundary per transport" is ported: it tests `resolveTranscriptTools` alone.
//! Every other test captures a request payload through `streamSimple` and checks how an API module (Anthropic
//! Messages, OpenAI Responses, OpenAI Completions, Kimi) serializes tool changes; those belong to the wave-3 API
//! shards.

use pi_ai::types::{Message, Tool};
use pi_ai::utils::transcript::{TranscriptToolSupport, TranscriptTools, resolve_transcript_tools};
use serde_json::{Value, json};

fn tool(name: &str) -> Tool {
	Tool {
		name: name.into(),
		description: format!("{name} tool"),
		parameters: json!({ "type": "object", "properties": {} }),
		..Tool::default()
	}
}

fn described(tool: Tool, description: &str) -> Tool {
	Tool {
		description: description.into(),
		..tool
	}
}

fn tool_json(tool: &Tool) -> Value {
	serde_json::to_value(tool).unwrap()
}

mod transcript_system_messages {
	use super::*;

	#[test]
	fn resolves_the_top_level_tool_boundary_per_transport() {
		let base_tool = tool("base_tool");
		let drop_tool = tool("drop_tool");
		let late_tool = tool("late_tool");
		let messages: Vec<Message> = serde_json::from_value(json!([
			{ "role": "system", "content": "base prompt", "toolsAdded": [tool_json(&base_tool), tool_json(&drop_tool)], "timestamp": 0 },
			{ "role": "system", "content": "", "toolsRemoved": [{ "name": "drop_tool" }], "timestamp": 1 },
			{ "role": "system", "content": "", "toolsAdded": [tool_json(&drop_tool)], "timestamp": 2 },
			{ "role": "system", "content": "", "toolsAdded": [tool_json(&described(late_tool.clone(), "x"))], "timestamp": 3 },
			{ "role": "system", "content": "", "toolsAdded": [tool_json(&described(late_tool.clone(), "y"))], "timestamp": 4 },
		]))
		.unwrap();

		assert_eq!(
			resolve_transcript_tools(&messages[..2], TranscriptToolSupport::Additions),
			TranscriptTools {
				request_tools: vec![base_tool.clone()],
				folded_system_messages: 2,
			}
		);
		// Kimi K3 ignores a removed tool that a later tool message declares again, so the re-added
		// drop_tool moves the boundary; the new late_tool after it stays anchored.
		assert_eq!(
			resolve_transcript_tools(&messages[..4], TranscriptToolSupport::Additions),
			TranscriptTools {
				request_tools: vec![base_tool.clone(), drop_tool.clone()],
				folded_system_messages: 3,
			}
		);
		// The redefinition of late_tool moves the boundary to its message.
		let all_tools = vec![base_tool, drop_tool, described(late_tool, "y")];
		assert_eq!(
			resolve_transcript_tools(&messages, TranscriptToolSupport::Additions),
			TranscriptTools {
				request_tools: all_tools.clone(),
				folded_system_messages: 5,
			}
		);
		assert_eq!(
			resolve_transcript_tools(&messages, TranscriptToolSupport::Changes),
			TranscriptTools {
				request_tools: vec![],
				folded_system_messages: 0,
			}
		);
		assert_eq!(
			resolve_transcript_tools(&messages, TranscriptToolSupport::None),
			TranscriptTools {
				request_tools: all_tools,
				folded_system_messages: 5,
			}
		);
	}
}
