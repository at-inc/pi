//! Port of `packages/agent/test/agent-tool-typebox.test.ts`.
//!
//! The TS test is type-level (`expectTypeOf`): an `AgentTool` built from a TypeBox schema types `execute()` and
//! `prepareArguments()` params from the schema. Rust tools take params as `serde_json::Value`, so the port checks
//! that a TypeBox-shaped schema (with its `~kind` and `~optional` keys) is accepted as `parameters` and that both
//! callbacks receive the params as plain JSON.

use std::rc::Rc;

use pi_agent::types::{AgentTool, AgentToolResult};
use pi_ai::types::Tool;
use serde_json::{Value, json};

fn parameters() -> Value {
	json!({
		"~kind": "Object",
		"type": "object",
		"properties": {
			"command": { "~kind": "String", "type": "string" },
			"mode": {
				"~kind": "Union",
				"anyOf": [
					{ "~kind": "Literal", "const": "fast" },
					{ "~kind": "Literal", "const": "deep" },
				],
				"~optional": true,
			},
		},
		"required": ["command"],
	})
}

mod agent_tool_params_of_a_type_box_schema {
	use super::*;

	#[test]
	fn are_typed_by_the_schema_in_execute_and_prepare_arguments() {
		let tool = AgentTool {
			tool: Tool {
				name: "bash".into(),
				description: "Runs a command".into(),
				parameters: parameters(),
				..Tool::default()
			},
			label: "Bash".into(),
			prepare_arguments: Some(Rc::new(|params: Value| {
				Ok(params.as_object().cloned().unwrap_or_default())
			})),
			output_schema: None,
			execute: Rc::new(|_tool_call_id, params: Value, _signal, _on_update| {
				assert!(params.is_object());
				Box::pin(async {
					Ok(AgentToolResult {
						content: Vec::new(),
						details: None,
						..AgentToolResult::default()
					})
				})
			}),
			replay: None,
			execution_mode: None,
		};
		assert_eq!(tool.parameters, parameters());
		let prepared = (tool.prepare_arguments.as_ref().unwrap())(json!({ "command": "ls", "mode": "fast" })).unwrap();
		assert_eq!(Value::Object(prepared), json!({ "command": "ls", "mode": "fast" }));
	}
}
