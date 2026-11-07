//! Port of `packages/ai/test/openai-completions-tool-result-images.test.ts`.

mod openai_completions_support;

use indexmap::IndexMap;
use openai_completions_support::{builtin_model, context, user_message};
use pi_ai::api::openai_completions::{ResolvedOpenAICompletionsCompat, convert_messages};
use pi_ai::providers::compat_schema::{CacheControlFormat, MaxTokensField, ThinkingFormat};
use pi_ai::types::{
	AssistantContent, AssistantMessage, ImageContent, Message, Model, ModelInputModality, SessionAffinityFormat,
	StopReason, TextContent, ToolCall, ToolResultMessage, UserContent, UserMessage, UserMessageContent,
};
use serde_json::{Value, json};

fn compat() -> ResolvedOpenAICompletionsCompat {
	ResolvedOpenAICompletionsCompat {
		supports_store: true,
		supports_developer_role: true,
		supports_reasoning_effort: true,
		supports_usage_in_streaming: true,
		supports_finish_reason: true,
		max_tokens_field: MaxTokensField::MaxCompletionTokens,
		requires_tool_result_name: false,
		requires_assistant_after_tool_result: false,
		requires_thinking_as_text: false,
		requires_reasoning_content_on_assistant_messages: false,
		thinking_format: ThinkingFormat::Openai,
		open_router_routing: json!({}),
		vercel_gateway_routing: json!({}),
		chat_template_kwargs: IndexMap::new(),
		chat_template_args: IndexMap::new(),
		zai_tool_stream: false,
		supports_thinking_token_budget: Some(false),
		thinking_token_budget_field: None,
		supports_strict_mode: true,
		supports_openai_grammar_tools: false,
		supports_mid_convo_system_messages: Some(false),
		supports_mid_convo_tool_additions: Some(false),
		cache_control_format: Some(CacheControlFormat::Anthropic),
		send_session_affinity_headers: false,
		session_affinity_format: SessionAffinityFormat::Openai,
		supports_long_cache_retention: true,
		vllm_priority: None,
	}
}

fn image_model() -> Model {
	let mut model = builtin_model("openai", "gpt-4o-mini");
	model.base.input = vec![ModelInputModality::Text, ModelInputModality::Image];
	model
}

fn text(text: &str) -> TextContent {
	TextContent {
		text: text.into(),
		..TextContent::default()
	}
}

fn image() -> ImageContent {
	ImageContent {
		data: "ZmFrZQ==".into(),
		mime_type: "image/png".into(),
		..ImageContent::default()
	}
}

fn build_tool_result(tool_call_id: &str, timestamp: f64) -> Message {
	Message::ToolResult(ToolResultMessage {
		tool_call_id: tool_call_id.into(),
		tool_name: "read".into(),
		content: vec![
			UserContent::Text(text("Read image file [image/png]")),
			UserContent::Image(image()),
		],
		is_error: false,
		timestamp,
		..ToolResultMessage::default()
	})
}

fn build_empty_tool_result(tool_call_id: &str, timestamp: f64, result: &str) -> Message {
	Message::ToolResult(ToolResultMessage {
		tool_call_id: tool_call_id.into(),
		tool_name: "bash".into(),
		content: vec![UserContent::Text(text(result))],
		is_error: false,
		timestamp,
		..ToolResultMessage::default()
	})
}

fn tool_call(id: &str, name: &str, arguments: Value) -> AssistantContent {
	AssistantContent::ToolCall(ToolCall {
		id: id.into(),
		name: name.into(),
		arguments: arguments.as_object().cloned().unwrap(),
		..ToolCall::default()
	})
}

fn assistant(model: &Model, content: Vec<AssistantContent>, timestamp: f64) -> Message {
	Message::Assistant(AssistantMessage {
		content,
		api: model.api.clone(),
		provider: model.provider.clone(),
		model: model.id.clone(),
		stop_reason: StopReason::ToolUse,
		timestamp,
		..AssistantMessage::default()
	})
}

fn user_at(text: &str, timestamp: f64) -> Message {
	let Message::User(mut message) = user_message(text) else {
		unreachable!()
	};
	message.timestamp = timestamp;
	Message::User(message)
}

mod openai_completions_convert_messages {
	use super::*;

	// Regression test for https://github.com/earendil-works/pi/issues/9797
	#[test]
	fn omits_empty_text_parts_from_user_messages_with_images() {
		let model = image_model();
		let context = context(vec![Message::User(UserMessage {
			content: UserMessageContent::Blocks(vec![UserContent::Text(text("")), UserContent::Image(image())]),
			timestamp: 0.0,
			..UserMessage::default()
		})]);

		assert_eq!(
			convert_messages(&model, &context, &compat(), None).unwrap(),
			vec![json!({
				"role": "user",
				"content": [{ "type": "image_url", "image_url": { "url": "data:image/png;base64,ZmFrZQ==" } }],
			})]
		);
	}

	#[test]
	fn batches_tool_result_images_after_consecutive_tool_results() {
		let model = image_model();
		let now = 1_000.0;
		let assistant_message = assistant(
			&model,
			vec![
				tool_call("tool-1", "read", json!({ "path": "img-1.png" })),
				tool_call("tool-2", "read", json!({ "path": "img-2.png" })),
			],
			now,
		);
		let context = context(vec![
			user_at("Read the images", now - 2.0),
			assistant_message,
			build_tool_result("tool-1", now + 1.0),
			build_tool_result("tool-2", now + 2.0),
		]);

		let messages = convert_messages(&model, &context, &compat(), None).unwrap();
		let roles: Vec<&str> = messages
			.iter()
			.map(|message| message["role"].as_str().unwrap())
			.collect();
		assert_eq!(roles, ["user", "assistant", "tool", "tool", "user"]);

		let image_message = messages.last().unwrap();
		assert_eq!(image_message["role"], "user");
		assert!(image_message["content"].is_array());

		let image_parts = image_message["content"]
			.as_array()
			.unwrap()
			.iter()
			.filter(|part| part["type"] == "image_url")
			.count();
		assert_eq!(image_parts, 2);
	}

	// Whitespace-only cases: https://github.com/earendil-works/pi/issues/8720
	#[test]
	fn uses_no_tool_output_placeholder_for_tool_result_text_without_images() {
		for result in ["", "\r\n", " \n\t"] {
			let model = image_model();
			let now = 1_000.0;
			let assistant_message = assistant(
				&model,
				vec![tool_call("tool-1", "bash", json!({ "command": "true" }))],
				now,
			);
			let context = context(vec![
				user_at("Run the command", now - 1.0),
				assistant_message,
				build_empty_tool_result("tool-1", now + 1.0, result),
			]);

			let messages = convert_messages(&model, &context, &compat(), None).unwrap();
			let tool_message = messages.iter().find(|message| message["role"] == "tool");
			let tool_message = tool_message.expect("a tool message");
			assert_eq!(tool_message["content"], "(no tool output)", "{result:?}");
			assert!(!tool_message["content"].as_str().unwrap().contains("see attached image"));
		}
	}
}
