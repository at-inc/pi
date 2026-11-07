//! Port of `packages/ai/test/openai-completions-cache-control-format.test.ts`.

mod openai_completions_support;

use openai_completions_support::{last_body, model, options, serve_chunks, user_message};
use pi_ai::api::openai_completions::stream;
use pi_ai::types::{
	AssistantContent, AssistantMessage, CacheRetention, Context, Message, Model, StopReason, TextContent, Tool,
	ToolCall, ToolResultMessage, UserContent,
};
use pi_ai::utils::transcript::normalize_context;
use pi_host::testing::{TestHost, run};
use serde_json::{Value, json};

async fn capture_payload(
	host: &TestHost,
	model: &Model,
	cache_retention: Option<CacheRetention>,
	messages: Option<Vec<Message>>,
) -> Value {
	serve_chunks(
		host,
		vec![json!({ "choices": [{ "delta": {}, "finish_reason": "stop" }] })],
	);
	let context = normalize_context(&Context {
		system_prompt: Some("System prompt".into()),
		messages: messages.unwrap_or_else(|| vec![user_message("Hello")]),
		tools: Some(vec![Tool {
			name: "read".into(),
			description: "Read a file".into(),
			parameters: json!({
				"type": "object",
				"properties": { "path": { "type": "string" } },
				"required": ["path"],
			}),
			..Tool::default()
		}]),
	});
	let mut request = options("test-key");
	request.cache_retention = cache_retention;
	stream(model, &context, Some(request)).result().await;
	last_body(host)
}

fn get_instruction_message(params: &Value) -> Option<&Value> {
	params["messages"]
		.as_array()
		.unwrap()
		.iter()
		.find(|message| message["role"] == "system" || message["role"] == "developer")
}

fn expect_anthropic_cache_markers(params: &Value) {
	let instruction_message = get_instruction_message(params).expect("an instruction message");
	assert!(instruction_message["content"].is_array());
	assert_eq!(
		instruction_message["content"][0]["cache_control"],
		json!({ "type": "ephemeral" })
	);
	assert_eq!(params["tools"].as_array().unwrap().len(), 1);
	assert_eq!(params["tools"][0]["cache_control"], json!({ "type": "ephemeral" }));
	let last_message = params["messages"].as_array().unwrap().last().unwrap();
	assert_eq!(last_message["role"], "user");
	assert!(last_message["content"].is_array());
	assert_eq!(
		last_message["content"][0]["cache_control"],
		json!({ "type": "ephemeral" })
	);
}

fn custom_qwen() -> Model {
	let mut model = model("custom-qwen", "openrouter", "https://example.com/v1");
	model.base.name = "Custom Qwen".into();
	model.reasoning = true;
	model.max_tokens = 32000.0;
	model.compat = json!({ "cacheControlFormat": "anthropic" }).as_object().cloned();
	model
}

fn batch_alias() -> Model {
	pi_ai::providers::all::get_builtin_model("openrouter", "anthropic/claude-fable-5.1:batch").expect("model")
}

mod openai_completions_cache_control_format {
	use super::*;

	#[test]
	fn applies_anthropic_style_cache_markers_when_model_compat_enables_them() {
		run(|host| async move {
			let params = capture_payload(&host, &custom_qwen(), None, None).await;
			expect_anthropic_cache_markers(&params);
		});
	}

	#[test]
	fn preserves_anthropic_style_cache_markers_for_openrouter_anthropic_batch_aliases() {
		run(|host| async move {
			let params = capture_payload(&host, &batch_alias(), None, None).await;
			expect_anthropic_cache_markers(&params);
		});
	}

	#[test]
	fn moves_the_conversation_cache_marker_to_a_tool_result() {
		run(|host| async move {
			let model = batch_alias();
			let params = capture_payload(
				&host,
				&model,
				None,
				Some(vec![
					user_message("Read the file"),
					Message::Assistant(AssistantMessage {
						content: vec![AssistantContent::ToolCall(ToolCall {
							id: "call_1".into(),
							name: "read".into(),
							arguments: json!({ "path": "README.md" }).as_object().cloned().unwrap(),
							..ToolCall::default()
						})],
						api: "openai-completions".into(),
						provider: "openrouter".into(),
						model: model.id.clone(),
						stop_reason: StopReason::ToolUse,
						..AssistantMessage::default()
					}),
					Message::ToolResult(ToolResultMessage {
						tool_call_id: "call_1".into(),
						tool_name: "read".into(),
						content: vec![UserContent::Text(TextContent {
							text: "file contents".into(),
							..TextContent::default()
						})],
						is_error: false,
						..ToolResultMessage::default()
					}),
				]),
			)
			.await;

			let messages = params["messages"].as_array().unwrap();
			let user = messages.iter().find(|message| message["role"] == "user").unwrap();
			assert_eq!(user["content"], "Read the file");
			let tool_message = messages.last().unwrap();
			assert_eq!(tool_message["role"], "tool");
			assert!(tool_message["content"].is_array());
			assert_eq!(
				tool_message["content"][0]["cache_control"],
				json!({ "type": "ephemeral" })
			);
		});
	}

	#[test]
	fn omits_anthropic_style_cache_markers_when_cache_retention_is_none() {
		run(|host| async move {
			let params = capture_payload(&host, &custom_qwen(), Some(CacheRetention::None), None).await;
			let instruction_message = get_instruction_message(&params).unwrap();
			assert!(!instruction_message["content"].is_array());
			assert_eq!(params["tools"][0].get("cache_control"), None);
			assert!(params["messages"].as_array().unwrap().last().unwrap()["content"].is_string());
		});
	}
}
