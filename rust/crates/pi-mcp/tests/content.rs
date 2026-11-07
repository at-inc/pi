//! Port of `packages/mcp/test/content.test.ts`.

use pi_mcp::protocol::content::{CallToolResult, ContentBlock, to_llm_content};
use serde_json::{Value, json};

fn blocks(value: Value) -> Vec<ContentBlock> {
	serde_json::from_value(value).unwrap()
}

fn llm(result: &CallToolResult) -> Value {
	serde_json::to_value(to_llm_content(result)).unwrap()
}

mod to_llm_content_tests {
	use super::*;

	#[test]
	fn passes_text_and_images_through_and_replaces_other_blocks_with_text() {
		let content = blocks(json!([
			{ "type": "text", "text": "hello", "annotations": { "priority": 1 } },
			{ "type": "image", "data": "aW1n", "mimeType": "image/png", "_meta": { "x": 1 } },
			{ "type": "audio", "data": "YXVk", "mimeType": "audio/wav" },
			{ "type": "resource_link", "uri": "file:///a.txt", "name": "a.txt" },
			{ "type": "resource", "resource": { "uri": "file:///b.txt", "text": "inline" } },
			{ "type": "resource", "resource": { "uri": "file:///c.png", "mimeType": "image/png", "blob": "Yw==" } },
			{ "type": "resource", "resource": { "uri": "file:///d.bin", "blob": "ZA==" } },
		]));
		assert_eq!(
			llm(&CallToolResult {
				content,
				..Default::default()
			}),
			json!([
				{ "type": "text", "text": "hello" },
				{ "type": "image", "data": "aW1n", "mimeType": "image/png" },
				{ "type": "text", "text": "[audio audio/wav omitted]" },
				{ "type": "text", "text": "a.txt: file:///a.txt" },
				{ "type": "text", "text": "inline" },
				{ "type": "image", "data": "Yw==", "mimeType": "image/png" },
				{ "type": "text", "text": "[binary resource file:///d.bin (unknown type) omitted]" },
			])
		);
	}

	#[test]
	fn falls_back_to_the_structured_content_as_json_when_there_are_no_blocks() {
		// #10416: structured content may be any JSON value.
		let structured = |value: Value| CallToolResult {
			structured_content: Some(value),
			..Default::default()
		};
		assert_eq!(
			llm(&structured(json!([1, 2]))),
			json!([{ "type": "text", "text": "[\n  1,\n  2\n]" }])
		);
		assert_eq!(
			llm(&structured(Value::Null)),
			json!([{ "type": "text", "text": "null" }])
		);
		assert_eq!(
			llm(&structured(json!({ "n": 1 }))),
			json!([{ "type": "text", "text": "{\n  \"n\": 1\n}" }])
		);
		assert_eq!(
			llm(&CallToolResult {
				content: blocks(json!([{ "type": "text", "text": "n=1" }])),
				structured_content: Some(json!({ "n": 1 })),
				..Default::default()
			}),
			json!([{ "type": "text", "text": "n=1" }])
		);
	}
}
