//! Port of `packages/ai/test/openai-completions-thinking-as-text.test.ts`.
//!
//! The local `node:http` server is the test host's fetch, which answers `POST /chat/completions` and 404s otherwise.

mod openai_completions_support;

use std::cell::RefCell;
use std::rc::Rc;

use indexmap::IndexMap;
use openai_completions_support::{collect_events, context, options, sse_response};
use pi_ai::api::openai_completions::{ResolvedOpenAICompletionsCompat, convert_messages, stream};
use pi_ai::providers::compat_schema::{MaxTokensField, ThinkingFormat};
use pi_ai::types::{
	AssistantContent, AssistantMessage, AssistantMessageEvent, BaseModel, Message, Model, ModelCost,
	ModelInputModality, SessionAffinityFormat, StopReason, TextContent, ThinkingContent, UserMessage,
	UserMessageContent,
};
use pi_host::HttpResponse;
use pi_host::testing::run;
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
		requires_thinking_as_text: true,
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
		cache_control_format: None,
		send_session_affinity_headers: false,
		session_affinity_format: SessionAffinityFormat::Openai,
		supports_long_cache_retention: true,
		vllm_priority: None,
	}
}

/// `compat` as the model literal carries it (`undefined` fields are absent).
fn model_compat() -> Value {
	json!({
		"supportsStore": true,
		"supportsDeveloperRole": true,
		"supportsReasoningEffort": true,
		"supportsUsageInStreaming": true,
		"supportsFinishReason": true,
		"maxTokensField": "max_completion_tokens",
		"requiresToolResultName": false,
		"requiresAssistantAfterToolResult": false,
		"requiresThinkingAsText": true,
		"requiresReasoningContentOnAssistantMessages": false,
		"thinkingFormat": "openai",
		"openRouterRouting": {},
		"vercelGatewayRouting": {},
		"chatTemplateKwargs": {},
		"chatTemplateArgs": {},
		"zaiToolStream": false,
		"supportsThinkingTokenBudget": false,
		"supportsStrictMode": true,
		"supportsOpenAIGrammarTools": false,
		"supportsMidConvoSystemMessages": false,
		"supportsMidConvoToolAdditions": false,
		"sendSessionAffinityHeaders": false,
		"sessionAffinityFormat": "openai",
		"supportsLongCacheRetention": true,
	})
}

fn build_model(base_url: &str) -> Model {
	Model {
		base: BaseModel {
			id: "repro-model".into(),
			name: "Repro Model".into(),
			api: "openai-completions".into(),
			provider: "repro-provider".into(),
			base_url: base_url.into(),
			input: vec![ModelInputModality::Text],
			cost: ModelCost::default(),
			..BaseModel::default()
		},
		reasoning: true,
		context_window: 128_000.0,
		max_tokens: 4096.0,
		compat: model_compat().as_object().cloned(),
		..Model::default()
	}
}

fn build_assistant(content: Vec<AssistantContent>) -> Message {
	Message::Assistant(AssistantMessage {
		content,
		api: "openai-completions".into(),
		provider: "repro-provider".into(),
		model: "repro-model".into(),
		stop_reason: StopReason::Stop,
		timestamp: 2.0,
		..AssistantMessage::default()
	})
}

fn user(text: &str, timestamp: f64) -> Message {
	Message::User(UserMessage {
		content: UserMessageContent::Text(text.into()),
		timestamp,
		..UserMessage::default()
	})
}

fn build_context(assistant: Message) -> Vec<Message> {
	vec![user("hello", 1.0), assistant, user("continue", 3.0)]
}

fn thinking(text: &str) -> AssistantContent {
	AssistantContent::Thinking(ThinkingContent {
		thinking: text.into(),
		..ThinkingContent::default()
	})
}

fn text(text: &str) -> AssistantContent {
	AssistantContent::Text(TextContent {
		text: text.into(),
		..TextContent::default()
	})
}

mod openai_completions_thinking_as_text_replay {
	use super::*;

	#[test]
	fn serializes_same_model_thinking_plus_text_replay_as_assistant_text_parts() {
		let messages = convert_messages(
			&build_model("http://127.0.0.1:1"),
			&context(build_context(build_assistant(vec![
				thinking("internal reasoning"),
				text("visible answer"),
			]))),
			&compat(),
			None,
		)
		.unwrap();

		assert_eq!(
			messages[1],
			json!({
				"role": "assistant",
				"content": [
					{ "type": "text", "text": "internal reasoning" },
					{ "type": "text", "text": "visible answer" },
				],
			})
		);
	}

	#[test]
	fn serializes_same_model_thinking_only_replay_as_assistant_text_parts() {
		let messages = convert_messages(
			&build_model("http://127.0.0.1:1"),
			&context(build_context(build_assistant(vec![thinking("internal reasoning")]))),
			&compat(),
			None,
		)
		.unwrap();

		assert_eq!(
			messages[1],
			json!({
				"role": "assistant",
				"content": [{ "type": "text", "text": "internal reasoning" }],
			})
		);
	}

	#[test]
	fn reaches_the_endpoint_when_replay_contains_both_thinking_and_text() {
		run(|host| async move {
			let request_bodies: Rc<RefCell<Vec<Value>>> = Rc::default();
			let recorded = request_bodies.clone();
			host.on_fetch(move |request| {
				if request.method != "POST" || request.url != "http://127.0.0.1:4100/chat/completions" {
					return Box::pin(async { Ok(HttpResponse::from_bytes(404, IndexMap::new(), "")) });
				}
				recorded
					.borrow_mut()
					.push(serde_json::from_slice(request.body.as_deref().unwrap()).unwrap());
				let response = sse_response(&[
					json!({
						"id": "chatcmpl-repro",
						"object": "chat.completion.chunk",
						"created": 0,
						"model": "repro-model",
						"choices": [{ "index": 0, "delta": { "role": "assistant", "content": "ok" }, "finish_reason": null }],
					}),
					json!({
						"id": "chatcmpl-repro",
						"object": "chat.completion.chunk",
						"created": 0,
						"model": "repro-model",
						"choices": [{ "index": 0, "delta": {}, "finish_reason": "stop" }],
						"usage": { "prompt_tokens": 1, "completion_tokens": 1 },
					}),
				]);
				Box::pin(async move { Ok(response) })
			});

			let events = collect_events(stream(
				&build_model("http://127.0.0.1:4100"),
				&context(build_context(build_assistant(vec![
					thinking("internal reasoning"),
					text("visible answer"),
				]))),
				Some(options("test-key")),
			))
			.await;

			let request_bodies = request_bodies.borrow();
			assert_eq!(request_bodies.len(), 1);
			assert_eq!(
				request_bodies[0]["messages"][1],
				json!({
					"role": "assistant",
					"content": [
						{ "type": "text", "text": "internal reasoning" },
						{ "type": "text", "text": "visible answer" },
					],
				})
			);
			assert!(matches!(events.last(), Some(AssistantMessageEvent::Done { .. })));
		});
	}
}
