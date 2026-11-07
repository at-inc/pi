//! Port of `packages/ai/test/message-types.test.ts`.
//!
//! The TS tests check `ToolResultMessage<TDetails>` at the type level. The Rust `ToolResultMessage` is not generic:
//! `details` is always a `JsonValue`, so non-JSON details cannot be built and the second test checks the field type.

use pi_ai::types::{Context, JsonValue, Message, ToolResultMessage};
use pi_host::js::json_stringify;
use serde_json::json;

fn base(details: Option<JsonValue>) -> ToolResultMessage {
	ToolResultMessage {
		tool_call_id: "call-1".into(),
		tool_name: "read".into(),
		details,
		timestamp: 1.0,
		..ToolResultMessage::default()
	}
}

mod message_json_types {
	use super::*;

	#[test]
	fn preserves_json_compatible_detail_types_without_requiring_index_signatures() {
		let object_message = base(Some(
			json!({ "path": "a.ts", "items": ["first"], "summary": { "lines": 3 } }),
		));
		let array_message = base(Some(json!(["a", "b"])));
		let readonly_array_message = base(Some(json!(["a", { "count": 1 }])));
		let primitive_message = base(Some(json!("diagnostic")));
		let null_message = base(Some(JsonValue::Null));
		let transcript_message = Message::ToolResult(object_message.clone());
		let readonly_transcript_message = Message::ToolResult(readonly_array_message.clone());
		let context = Context {
			system_prompt: None,
			messages: vec![transcript_message.clone(), readonly_transcript_message.clone()],
			tools: None,
		};

		assert_eq!(object_message.details.as_ref().unwrap()["summary"]["lines"], json!(3));
		assert_eq!(array_message.details, Some(json!(["a", "b"])));
		assert_eq!(readonly_array_message.details, Some(json!(["a", { "count": 1 }])));
		assert_eq!(primitive_message.details, Some(json!("diagnostic")));
		assert_eq!(null_message.details, Some(JsonValue::Null));
		assert_eq!(transcript_message, Message::ToolResult(object_message));
		assert_eq!(readonly_transcript_message, Message::ToolResult(readonly_array_message));
		assert_eq!(context.messages, vec![transcript_message, readonly_transcript_message]);

		let serialized = json_stringify(&Message::ToolResult(null_message));
		assert_eq!(
			serialized,
			r#"{"role":"toolResult","toolCallId":"call-1","toolName":"read","content":[],"details":null,"isError":false,"timestamp":1}"#
		);
		let parsed: Message = serde_json::from_str(&serialized).unwrap();
		let Message::ToolResult(parsed) = parsed else {
			panic!("expected a tool result");
		};
		assert_eq!(parsed.details, Some(JsonValue::Null));
	}

	#[test]
	fn rejects_non_json_detail_types() {
		fn details_type(message: &ToolResultMessage) -> &Option<JsonValue> {
			&message.details
		}
		assert_eq!(details_type(&base(None)), &None);
	}
}
