//! Port of `packages/ai/test/openai-completions-tool-choice.test.ts`.
//!
//! The mocked `create` records its params and replays `mockState.chunks` (or one `stop` chunk); the port serves the same
//! chunks as SSE through the test host and reads the params from `onPayload`, falling back to the request body.

mod openai_completions_support;

use indexmap::IndexMap;
use openai_completions_support::{builtin_model, capture_payload, context, last_body, model, serve_chunks};
use pi_ai::api::openai_completions::{ResolvedOpenAICompletionsCompat, convert_messages};
use pi_ai::compat::{stream, stream_simple};
use pi_ai::providers::all::get_builtin_model;
use pi_ai::providers::compat_schema::{MaxTokensField, ThinkingFormat};
use pi_ai::types::{
	AssistantContent, AssistantMessage, AssistantMessageEvent, Context, Message, Model, ProviderStreamOptions,
	SessionAffinityFormat, SimpleStreamOptions, StopReason, TextContent, ThinkingContent, ThinkingLevel, Tool,
	ToolCall, ToolChoice, ToolResultMessage, UserContent, UserMessage, UserMessageContent,
};
use pi_host::testing::{TestHost, run};
use serde_json::{Map, Value, json};

fn default_chunk() -> Value {
	json!({
		"choices": [{ "delta": {}, "finish_reason": "stop" }],
		"usage": {
			"prompt_tokens": 1,
			"completion_tokens": 1,
			"prompt_tokens_details": { "cached_tokens": 0 },
			"completion_tokens_details": { "reasoning_tokens": 0 },
		},
	})
}

fn local_openai_completions_model(id: &str, name: &str) -> Model {
	let mut model = model(id, "local-vllm", "http://localhost:8000/v1");
	model.base.name = name.into();
	model.reasoning = true;
	model.max_tokens = 8192.0;
	model
}

fn builtin(provider: &str, id: &str) -> Model {
	get_builtin_model(provider, id).unwrap_or_else(|| panic!("built-in model {provider}/{id}"))
}

fn compat_value(model: &Model, key: &str) -> Option<Value> {
	model.compat.as_ref().and_then(|compat| compat.get(key)).cloned()
}

fn tool(value: Value) -> Tool {
	serde_json::from_value(value).unwrap()
}

/// `Type.Object(properties)` with every property required.
fn object_schema(properties: Value) -> Value {
	let required: Vec<Value> = properties
		.as_object()
		.unwrap()
		.keys()
		.map(|key| Value::String(key.clone()))
		.collect();
	json!({ "type": "object", "properties": properties, "required": required })
}

fn ping_tool() -> Tool {
	tool(json!({
		"name": "ping",
		"description": "Ping tool",
		"parameters": object_schema(json!({ "ok": { "type": "boolean" } })),
	}))
}

fn prefer_strict_ping_tool() -> Tool {
	tool(json!({
		"name": "ping",
		"description": "Ping tool",
		"parameters": {
			"type": "object",
			"properties": { "required": { "type": "string" }, "optional": { "type": "string" } },
			"required": ["required"],
		},
		"constrainedSampling": { "type": "json_schema", "strict": "prefer" },
	}))
}

fn user(text: &str) -> Message {
	Message::User(UserMessage {
		content: UserMessageContent::Text(text.into()),
		timestamp: pi_host::now_ms(),
		..UserMessage::default()
	})
}

fn messages(messages: Vec<Message>) -> Context {
	Context {
		messages,
		..Context::default()
	}
}

fn with_tools(messages: Vec<Message>, tools: Vec<Tool>) -> Context {
	Context {
		messages,
		tools: Some(tools),
		..Context::default()
	}
}

fn with_system_prompt(text: &str) -> Context {
	Context {
		system_prompt: Some("Follow instructions.".into()),
		messages: vec![user(text)],
		..Context::default()
	}
}

fn simple(reasoning: Option<ThinkingLevel>) -> SimpleStreamOptions {
	let mut options = SimpleStreamOptions::default();
	options.base.api_key = Some("test".into());
	options.reasoning = reasoning;
	options
}

/// Streams with an `onPayload` that records the params; the mocked stream is one `stop` chunk.
async fn capture(host: &TestHost, model: &Model, context: Context, mut options: SimpleStreamOptions) -> Value {
	serve_chunks(host, vec![default_chunk()]);
	let (on_payload, payload) = capture_payload(None);
	options.base.on_payload = Some(on_payload);
	stream_simple(model, &context, Some(options))
		.unwrap()
		.result()
		.await
		.unwrap();
	let payload = payload.borrow().clone();
	payload.unwrap_or_else(|| last_body(host))
}

/// `captureSimpleParams(model, reasoning)`.
async fn capture_simple_params(host: &TestHost, model: &Model, reasoning: Option<ThinkingLevel>) -> Value {
	capture(host, model, messages(vec![user("Hi")]), simple(reasoning)).await
}

/// Streams the mocked `chunks` and returns every event and the final message.
async fn respond(
	host: &TestHost,
	chunks: Vec<Value>,
	model: &Model,
	context: Context,
) -> (Vec<AssistantMessageEvent>, AssistantMessage) {
	serve_chunks(host, chunks);
	let stream = stream_simple(model, &context, Some(simple(None))).unwrap();
	let mut events = Vec::new();
	while let Some(event) = stream.next().await {
		events.push(event);
	}
	(events, stream.result().await.unwrap())
}

fn event_type(event: &AssistantMessageEvent) -> &'static str {
	match event {
		AssistantMessageEvent::Start { .. } => "start",
		AssistantMessageEvent::TextStart { .. } => "text_start",
		AssistantMessageEvent::TextDelta { .. } => "text_delta",
		AssistantMessageEvent::TextEnd { .. } => "text_end",
		AssistantMessageEvent::ThinkingStart { .. } => "thinking_start",
		AssistantMessageEvent::ThinkingDelta { .. } => "thinking_delta",
		AssistantMessageEvent::ThinkingEnd { .. } => "thinking_end",
		AssistantMessageEvent::ToolcallStart { .. } => "toolcall_start",
		AssistantMessageEvent::ToolcallDelta { .. } => "toolcall_delta",
		AssistantMessageEvent::ToolcallEnd { .. } => "toolcall_end",
		AssistantMessageEvent::Done { .. } => "done",
		AssistantMessageEvent::Error { .. } => "error",
	}
}

fn tool_call_content_index(event: &AssistantMessageEvent) -> Option<usize> {
	match event {
		AssistantMessageEvent::ToolcallStart { content_index, .. }
		| AssistantMessageEvent::ToolcallDelta { content_index, .. }
		| AssistantMessageEvent::ToolcallEnd { content_index, .. } => Some(*content_index),
		_ => None,
	}
}

fn content_json(message: &AssistantMessage) -> Value {
	serde_json::to_value(&message.content).unwrap()
}

fn tool_call_message(provider: &str, model: &str, content: Vec<AssistantContent>, stop_reason: StopReason) -> Message {
	Message::Assistant(AssistantMessage {
		api: "openai-completions".into(),
		provider: provider.into(),
		model: model.into(),
		content,
		stop_reason,
		timestamp: pi_host::now_ms(),
		..AssistantMessage::default()
	})
}

fn read_call() -> AssistantContent {
	AssistantContent::ToolCall(ToolCall {
		id: "call_1".into(),
		name: "read".into(),
		arguments: json!({ "path": "README.md" }).as_object().cloned().unwrap(),
		..ToolCall::default()
	})
}

fn thinking(text: &str, signature: &str) -> AssistantContent {
	AssistantContent::Thinking(ThinkingContent {
		thinking: text.into(),
		thinking_signature: Some(signature.into()),
		..ThinkingContent::default()
	})
}

fn read_result() -> Message {
	Message::ToolResult(ToolResultMessage {
		tool_call_id: "call_1".into(),
		tool_name: "read".into(),
		content: vec![UserContent::Text(TextContent {
			text: "contents".into(),
			..TextContent::default()
		})],
		is_error: false,
		timestamp: pi_host::now_ms(),
		..ToolResultMessage::default()
	})
}

fn assistant_payload(params: &Value) -> Value {
	params["messages"]
		.as_array()
		.and_then(|messages| messages.iter().find(|message| message["role"] == "assistant"))
		.cloned()
		.expect("an assistant message")
}

fn read_tool() -> Tool {
	tool(json!({
		"name": "read",
		"description": "Read a file",
		"parameters": object_schema(json!({ "path": { "type": "string" } })),
	}))
}

mod openai_completions_tool_choice {
	use super::*;

	// The vitest file casts the options to pass `"required"`, which the provider-neutral `ToolChoice` type does not hold; the
	// typed Rust options cannot carry it either, so the port passes it the untyped way, as a `toolChoice` extra key.
	#[test]
	fn forwards_tool_choice_from_simple_options_to_payload() {
		run(|host| async move {
			serve_chunks(&host, vec![default_chunk()]);
			let (on_payload, payload) = capture_payload(None);
			let mut options = ProviderStreamOptions::default();
			options.base.api_key = Some("test".into());
			options.base.on_payload = Some(on_payload);
			options.extra.insert("toolChoice".into(), json!("required"));
			stream(
				&builtin_model("openai", "gpt-4o-mini"),
				&with_tools(vec![user("Call ping with ok=true")], vec![ping_tool()]),
				Some(options),
			)
			.unwrap()
			.result()
			.await
			.unwrap();
			let params = payload.borrow().clone().unwrap_or_else(|| last_body(&host));
			assert_eq!(params["tool_choice"], json!("required"));
			assert!(params["tools"].as_array().is_some_and(|tools| !tools.is_empty()));
		});
	}

	// The same call through `streamSimple`, as the wasm boundary passes it: the unchecked value rides in `extra`.
	#[test]
	fn forwards_untyped_tool_choice_from_simple_options_to_payload() {
		run(|host| async move {
			let mut options = simple(None);
			options.extra.insert("toolChoice".into(), json!("required"));
			let params = capture(
				&host,
				&builtin_model("openai", "gpt-4o-mini"),
				with_tools(vec![user("Call ping with ok=true")], vec![ping_tool()]),
				options,
			)
			.await;
			assert_eq!(params["tool_choice"], json!("required"));
		});
	}

	#[test]
	fn includes_tool_choice_when_no_tools_are_provided() {
		run(|host| async move {
			let mut options = simple(None);
			options.tool_choice = Some(ToolChoice::None);
			let params = capture(
				&host,
				&builtin_model("openai", "gpt-4o-mini"),
				messages(vec![user("Summarize the conversation")]),
				options,
			)
			.await;
			assert_eq!(params["tool_choice"], json!("none"));
			assert!(!params.as_object().unwrap().contains_key("tools"));
		});
	}

	#[test]
	fn omits_strict_when_compat_disables_strict_mode() {
		run(|host| async move {
			let mut model = builtin_model("openai", "gpt-4o-mini");
			model.compat = json!({ "supportsStrictMode": false }).as_object().cloned();
			let params = capture(
				&host,
				&model,
				with_tools(vec![user("Call ping with ok=true")], vec![ping_tool()]),
				simple(None),
			)
			.await;
			let tool = params["tools"][0]["function"].as_object().expect("a function tool");
			assert!(!tool.contains_key("strict"));
		});
	}

	#[test]
	fn defaults_unknown_openai_compatible_endpoints_to_non_strict_tools() {
		// Regression test for #9816.
		run(|host| async move {
			let model = local_openai_completions_model("local-model", "Local Model");
			let params = capture(
				&host,
				&model,
				with_tools(vec![user("Call ping")], vec![prefer_strict_ping_tool()]),
				simple(None),
			)
			.await;
			let function_tool = &params["tools"][0]["function"];
			assert!(!function_tool.as_object().unwrap().contains_key("strict"));
			assert_eq!(function_tool["parameters"]["required"], json!(["required"]));
		});
	}

	#[test]
	fn preserves_strict_tools_for_capable_built_in_chat_completions_models() {
		run(|host| async move {
			let model = builtin("groq", "openai/gpt-oss-20b");
			assert_eq!(compat_value(&model, "supportsStrictMode"), Some(json!(true)));
			let params = capture(
				&host,
				&model,
				with_tools(vec![user("Call ping")], vec![prefer_strict_ping_tool()]),
				simple(None),
			)
			.await;
			let function_tool = &params["tools"][0]["function"];
			assert_eq!(function_tool["strict"], json!(true));
			assert_eq!(function_tool["parameters"]["required"], json!(["required", "optional"]));
		});
	}

	#[test]
	fn maps_groq_qwen_reasoning_levels_to_default_reasoning_effort() {
		run(|host| async move {
			let params =
				capture_simple_params(&host, &builtin("groq", "qwen/qwen3.6-27b"), Some(ThinkingLevel::Medium)).await;
			assert_eq!(params["reasoning_effort"], json!("default"));
		});
	}

	#[test]
	fn keeps_normal_reasoning_effort_for_groq_models_without_compat_mapping() {
		run(|host| async move {
			let params = capture_simple_params(
				&host,
				&builtin("groq", "openai/gpt-oss-20b"),
				Some(ThinkingLevel::Medium),
			)
			.await;
			assert_eq!(params["reasoning_effort"], json!("medium"));
		});
	}

	#[test]
	fn enables_tool_stream_for_supported_z_ai_models_with_tools() {
		run(|host| async move {
			let params = capture(
				&host,
				&builtin("zai", "glm-5.2"),
				with_tools(vec![user("Call ping with ok=true")], vec![ping_tool()]),
				simple(None),
			)
			.await;
			assert_eq!(params["tool_stream"], json!(true));
		});
	}

	#[test]
	fn stores_z_ai_tool_stream_support_in_model_compat_metadata() {
		for id in ["glm-4.7", "glm-4.7", "glm-5-turbo", "glm-5.2"] {
			assert_eq!(
				compat_value(&builtin("zai", id), "zaiToolStream"),
				Some(json!(true)),
				"{id}"
			);
		}
	}

	#[test]
	fn stores_z_ai_effort_metadata() {
		for model_id in ["glm-5.2", "glm-5.2-highspeed"] {
			let model = builtin("zai", model_id);
			assert_eq!(compat_value(&model, "supportsReasoningEffort"), Some(json!(true)));
			assert_eq!(
				serde_json::to_value(&model.thinking_level_map).unwrap(),
				json!({
					"off": "none",
					"minimal": null,
					"low": null,
					"medium": null,
					"high": "high",
					"xhigh": null,
					"max": "max",
				})
			);
		}

		for provider in ["zai", "zai-coding-cn"] {
			let glm53 = builtin(provider, "glm-5.3");
			assert_eq!(compat_value(&glm53, "supportsReasoningEffort"), Some(json!(true)));
			assert_eq!(
				serde_json::to_value(&glm53.thinking_level_map).unwrap(),
				json!({
					"off": null,
					"minimal": null,
					"low": "low",
					"medium": null,
					"high": "high",
					"xhigh": null,
					"max": "max",
				})
			);
		}
	}

	#[test]
	fn maps_z_ai_glm_5_2_thinking_levels_to_reasoning_effort() {
		run(|host| async move {
			let model = builtin("zai", "glm-5.2");
			for (reasoning, effort) in [
				(ThinkingLevel::Low, "high"),
				(ThinkingLevel::Medium, "high"),
				(ThinkingLevel::High, "high"),
				(ThinkingLevel::Max, "max"),
			] {
				let params = capture_simple_params(&host, &model, Some(reasoning)).await;
				assert_eq!(
					params["thinking"],
					json!({ "type": "enabled", "clear_thinking": false })
				);
				assert_eq!(params["reasoning_effort"], json!(effort));
			}
		});
	}

	#[test]
	fn preserves_z_ai_thinking_when_replaying_reasoning_content() {
		run(|host| async move {
			let assistant_message = tool_call_message(
				"zai",
				"glm-5.2",
				vec![thinking("prior reasoning", "reasoning_content"), read_call()],
				StopReason::ToolUse,
			);
			let params = capture(
				&host,
				&builtin("zai", "glm-5.2"),
				messages(vec![
					user("Read README.md"),
					assistant_message,
					read_result(),
					user("Continue"),
				]),
				simple(Some(ThinkingLevel::High)),
			)
			.await;
			assert_eq!(
				assistant_payload(&params)["reasoning_content"],
				json!("prior reasoning")
			);
			assert_eq!(
				params["thinking"],
				json!({ "type": "enabled", "clear_thinking": false })
			);
		});
	}

	#[test]
	fn omits_z_ai_glm_5_2_reasoning_effort_when_thinking_is_off() {
		run(|host| async move {
			let params = capture_simple_params(&host, &builtin("zai", "glm-5.2"), None).await;
			assert_eq!(params["thinking"], json!({ "type": "disabled" }));
			assert_eq!(params.get("reasoning_effort"), None);
		});
	}

	#[test]
	fn respects_explicit_z_ai_tool_stream_compat_override() {
		run(|host| async move {
			let mut model = builtin("zai", "glm-5.2");
			model
				.compat
				.get_or_insert_with(Map::new)
				.insert("zaiToolStream".into(), json!(true));
			let params = capture(
				&host,
				&model,
				with_tools(vec![user("Call ping with ok=true")], vec![ping_tool()]),
				simple(None),
			)
			.await;
			assert_eq!(params["tool_stream"], json!(true));
		});
	}

	#[test]
	fn omits_tool_stream_when_no_tools_are_provided() {
		run(|host| async move {
			let params = capture_simple_params(&host, &builtin("zai", "glm-5.2"), None).await;
			assert_eq!(params.get("tool_stream"), None);
		});
	}

	#[test]
	fn maps_non_standard_provider_finish_reason_values_to_stop_reason_error() {
		run(|host| async move {
			let chunks = vec![
				json!({ "choices": [{ "delta": { "content": "partial" }, "finish_reason": null }] }),
				json!({
					"choices": [{ "delta": {}, "finish_reason": "network_error" }],
					"usage": {
						"prompt_tokens": 1,
						"completion_tokens": 1,
						"prompt_tokens_details": { "cached_tokens": 0 },
						"completion_tokens_details": { "reasoning_tokens": 0 },
					},
				}),
			];
			let (_, response) = respond(&host, chunks, &builtin("zai", "glm-5.2"), messages(vec![user("Hi")])).await;
			assert_eq!(response.stop_reason, StopReason::Error);
			assert_eq!(
				response.error_message.as_deref(),
				Some("Provider finish_reason: network_error")
			);
		});
	}

	#[test]
	fn ignores_null_stream_chunks_from_openai_compatible_providers() {
		run(|host| async move {
			let chunks = vec![
				Value::Null,
				json!({ "id": "chatcmpl-test", "choices": [{ "delta": { "content": "OK" }, "finish_reason": null }] }),
				json!({
					"id": "chatcmpl-test",
					"choices": [{ "delta": {}, "finish_reason": "stop" }],
					"usage": {
						"prompt_tokens": 3,
						"completion_tokens": 1,
						"prompt_tokens_details": { "cached_tokens": 0 },
						"completion_tokens_details": { "reasoning_tokens": 0 },
					},
				}),
			];
			let (_, response) = respond(
				&host,
				chunks,
				&builtin_model("openai", "gpt-4o-mini"),
				messages(vec![user("Reply with exactly OK")]),
			)
			.await;
			assert_eq!(response.stop_reason, StopReason::Stop);
			assert_eq!(response.error_message, None);
			assert_eq!(response.response_id.as_deref(), Some("chatcmpl-test"));
			assert_eq!(response.usage.total_tokens, 4.0);
			assert_eq!(content_json(&response), json!([{ "type": "text", "text": "OK" }]));
		});
	}

	#[test]
	fn errors_when_a_stream_ends_after_only_null_finish_reason_chunks() {
		run(|host| async move {
			let chunk = json!({
				"id": "chatcmpl-truncated",
				"choices": [{ "delta": { "content": "partial answer" }, "finish_reason": null }],
			});
			let (_, response) = respond(
				&host,
				vec![chunk.clone(), chunk],
				&builtin_model("openai", "gpt-4o-mini"),
				messages(vec![user("Reply with a longer sentence")]),
			)
			.await;
			assert_eq!(response.stop_reason, StopReason::Error);
			assert_eq!(
				response.error_message.as_deref(),
				Some("Stream ended without finish_reason")
			);
		});
	}

	#[test]
	fn accepts_streams_without_finish_reason_when_compat_disables_it() {
		run(|host| async move {
			let mut model = builtin_model("openai", "gpt-4o-mini");
			model.compat = json!({ "supportsFinishReason": false }).as_object().cloned();
			let chunks = vec![json!({
				"id": "chatcmpl-no-finish-reason",
				"choices": [{ "delta": { "content": "complete answer" }, "finish_reason": null }],
			})];
			let (_, response) = respond(
				&host,
				chunks,
				&model,
				messages(vec![user("Reply with a complete answer")]),
			)
			.await;
			assert_eq!(response.stop_reason, StopReason::Stop);
			assert_eq!(response.error_message, None);
			assert_eq!(
				content_json(&response),
				json!([{ "type": "text", "text": "complete answer" }])
			);
		});
	}

	#[test]
	fn ignores_empty_custom_objects_on_function_tool_call_deltas() {
		run(|host| async move {
			let chunks = vec![json!({
				"id": "chatcmpl-empty-custom",
				"choices": [{
					"delta": {
						"tool_calls": [{
							"index": 0,
							"id": "call_1",
							"type": "function",
							"function": { "name": "read", "arguments": "{\"path\":\"README.md\"}" },
							"custom": {},
						}],
					},
					"finish_reason": "tool_calls",
				}],
			})];
			let (_, response) = respond(
				&host,
				chunks,
				&builtin_model("openai", "gpt-4o-mini"),
				with_tools(vec![user("Read README.md")], vec![read_tool()]),
			)
			.await;
			assert_eq!(
				content_json(&response),
				json!([{ "type": "toolCall", "id": "call_1", "name": "read", "arguments": { "path": "README.md" } }])
			);
		});
	}

	fn tool_call_chunk(tool_calls: Value, finish_reason: Option<&str>) -> Value {
		json!({
			"id": "chatcmpl-kimi-bad-stream",
			"choices": [{ "delta": { "tool_calls": tool_calls }, "finish_reason": finish_reason }],
		})
	}

	#[test]
	fn coalesces_tool_call_deltas_by_stable_index_when_provider_mutates_ids_mid_stream() {
		run(|host| async move {
			let mut last = tool_call_chunk(
				json!([{
					"index": 0,
					"id": "chatcmpl-tool-b",
					"type": "function",
					"function": { "name": null, "arguments": ".md\"}" },
				}]),
				Some("tool_calls"),
			);
			last["usage"] = json!({
				"prompt_tokens": 10,
				"completion_tokens": 5,
				"prompt_tokens_details": { "cached_tokens": 0 },
				"completion_tokens_details": { "reasoning_tokens": 0 },
			});
			let chunks = vec![
				tool_call_chunk(
					json!([{
						"index": 0,
						"id": "functions.read:0",
						"type": "function",
						"function": { "name": "read", "arguments": "" },
					}]),
					None,
				),
				tool_call_chunk(
					json!([{
						"index": 0,
						"id": "chatcmpl-tool-a",
						"type": "function",
						"function": { "name": null, "arguments": "{\"path\":\"README" },
					}]),
					None,
				),
				last,
			];
			let (events, response) = respond(
				&host,
				chunks,
				&builtin_model("openai", "gpt-4o-mini"),
				with_tools(vec![user("Read README.md")], vec![read_tool()]),
			)
			.await;

			let tool_call_content_indexes: Vec<usize> = events.iter().filter_map(tool_call_content_index).collect();
			assert_eq!(response.stop_reason, StopReason::ToolUse);
			assert_eq!(tool_call_content_indexes, vec![0, 0, 0, 0, 0]);
			assert_eq!(response.content.len(), 1);
			let tool_call = serde_json::to_value(&response.content[0]).unwrap();
			assert_eq!(tool_call["type"], json!("toolCall"));
			assert_eq!(tool_call["id"], json!("functions.read:0"));
			assert_eq!(tool_call["name"], json!("read"));
			assert_eq!(tool_call["arguments"], json!({ "path": "README.md" }));
			assert!(tool_call.get("streamIndex").is_none());
			assert!(tool_call.get("partialArgs").is_none());
		});
	}

	#[test]
	fn accumulates_mixed_content_reasoning_and_parallel_tool_call_deltas_independently() {
		run(|host| async move {
			let chunks = vec![
				json!({
					"id": "chatcmpl-mixed-deltas",
					"choices": [{
						"delta": {
							"content": "answer 1",
							"reasoning_content": "think 1",
							"tool_calls": [
								{
									"index": 0,
									"id": "tc_read_initial",
									"type": "function",
									"function": { "name": "read", "arguments": "{\"path\":\"README" },
								},
								{
									"index": 1,
									"id": "tc_grep_initial",
									"type": "function",
									"function": { "name": "grep", "arguments": "{\"pattern\":\"TODO" },
								},
								{
									"id": "tc_list_no_index",
									"type": "function",
									"function": { "name": "list", "arguments": "{\"path\":\"packages" },
								},
								{
									"id": "tc_write_no_index",
									"type": "function",
									"function": { "name": "write", "arguments": "{\"path\":\"out" },
								},
							],
						},
						"finish_reason": null,
					}],
				}),
				json!({
					"id": "chatcmpl-mixed-deltas",
					"choices": [{
						"delta": {
							"content": " answer 2",
							"tool_calls": [
								{
									"index": 1,
									"id": "tc_grep_changed",
									"type": "function",
									"function": { "arguments": "\",\"path\":\"src" },
								},
								{
									"id": "tc_write_no_index",
									"type": "function",
									"function": { "arguments": ".txt\",\"content\":\"ok\"}" },
								},
								{
									"id": "tc_list_no_index",
									"type": "function",
									"function": { "arguments": "/ai\"}" },
								},
							],
						},
						"finish_reason": null,
					}],
				}),
				json!({
					"id": "chatcmpl-mixed-deltas",
					"choices": [{
						"delta": {
							"content": "\n",
							"reasoning_content": " think 2",
							"tool_calls": [
								{
									"index": 0,
									"id": "tc_read_changed",
									"type": "function",
									"function": { "arguments": ".md\"}" },
								},
								{
									"index": 1,
									"type": "function",
									"function": { "arguments": "\"}" },
								},
							],
						},
						"finish_reason": "tool_calls",
					}],
					"usage": {
						"prompt_tokens": 10,
						"completion_tokens": 8,
						"prompt_tokens_details": { "cached_tokens": 0 },
						"completion_tokens_details": { "reasoning_tokens": 2 },
					},
				}),
			];
			let tools = vec![
				read_tool(),
				tool(json!({
					"name": "grep",
					"description": "Search a file",
					"parameters": object_schema(json!({ "pattern": { "type": "string" }, "path": { "type": "string" } })),
				})),
				tool(json!({
					"name": "list",
					"description": "List a directory",
					"parameters": object_schema(json!({ "path": { "type": "string" } })),
				})),
				tool(json!({
					"name": "write",
					"description": "Write a file",
					"parameters": object_schema(json!({ "path": { "type": "string" }, "content": { "type": "string" } })),
				})),
			];
			let (events, response) = respond(
				&host,
				chunks,
				&builtin_model("openai", "gpt-4o-mini"),
				with_tools(vec![user("Think, answer, and use tools.")], tools),
			)
			.await;

			let event_types: Vec<&str> = events.iter().map(event_type).collect();
			let mut tool_events_by_content_index: IndexMap<usize, Vec<&str>> = IndexMap::new();
			for event in &events {
				if let Some(content_index) = tool_call_content_index(event) {
					tool_events_by_content_index
						.entry(content_index)
						.or_default()
						.push(event_type(event));
				}
			}
			let count = |kind: &str| event_types.iter().filter(|event_type| **event_type == kind).count();

			assert_eq!(response.stop_reason, StopReason::ToolUse);
			assert_eq!(count("text_start"), 1);
			assert_eq!(count("text_delta"), 3);
			assert_eq!(count("text_end"), 1);
			assert_eq!(count("thinking_start"), 1);
			assert_eq!(count("thinking_delta"), 2);
			assert_eq!(count("thinking_end"), 1);
			assert_eq!(count("toolcall_start"), 4);
			assert_eq!(count("toolcall_delta"), 9);
			assert_eq!(count("toolcall_end"), 4);
			assert_eq!(
				tool_events_by_content_index.get(&2),
				Some(&vec![
					"toolcall_start",
					"toolcall_delta",
					"toolcall_delta",
					"toolcall_end"
				])
			);
			assert_eq!(
				tool_events_by_content_index.get(&3),
				Some(&vec![
					"toolcall_start",
					"toolcall_delta",
					"toolcall_delta",
					"toolcall_delta",
					"toolcall_end"
				])
			);
			assert_eq!(
				tool_events_by_content_index.get(&4),
				Some(&vec![
					"toolcall_start",
					"toolcall_delta",
					"toolcall_delta",
					"toolcall_end"
				])
			);
			assert_eq!(
				tool_events_by_content_index.get(&5),
				Some(&vec![
					"toolcall_start",
					"toolcall_delta",
					"toolcall_delta",
					"toolcall_end"
				])
			);

			let content = content_json(&response);
			assert_eq!(response.content.len(), 6);
			assert_eq!(
				content[0],
				json!({ "type": "thinking", "thinking": "think 1 think 2", "thinkingSignature": "reasoning_content" })
			);
			assert_eq!(content[1], json!({ "type": "text", "text": "answer 1 answer 2\n" }));
			for (index, id, name, arguments) in [
				(2, "tc_read_initial", "read", json!({ "path": "README.md" })),
				(
					3,
					"tc_grep_initial",
					"grep",
					json!({ "pattern": "TODO", "path": "src" }),
				),
				(4, "tc_list_no_index", "list", json!({ "path": "packages/ai" })),
				(
					5,
					"tc_write_no_index",
					"write",
					json!({ "path": "out.txt", "content": "ok" }),
				),
			] {
				let call = &content[index];
				assert_eq!(call["type"], json!("toolCall"));
				assert_eq!(call["id"], json!(id));
				assert_eq!(call["name"], json!(name));
				assert_eq!(call["arguments"], arguments);
				assert!(call.get("streamIndex").is_none());
				assert!(call.get("partialArgs").is_none());
			}
		});
	}

	#[test]
	fn uses_system_messages_for_non_openai_anthropic_openrouter_reasoning_model_instructions() {
		run(|host| async move {
			let params = capture(
				&host,
				&builtin("openrouter", "deepseek/deepseek-v4-pro"),
				with_system_prompt("Hi"),
				simple(None),
			)
			.await;
			assert_eq!(params["messages"][0]["role"], json!("system"));
		});
	}

	#[test]
	fn keeps_developer_messages_for_openai_and_anthropic_openrouter_batch_instructions() {
		run(|host| async move {
			for model in [
				builtin("openrouter", "openai/gpt-5.2-codex"),
				builtin("openrouter", "anthropic/claude-fable-5.1:batch"),
			] {
				let params = capture(&host, &model, with_system_prompt("Hi"), simple(None)).await;
				assert_eq!(params["messages"][0]["role"], json!("developer"));
			}
		});
	}

	#[test]
	fn keeps_developer_messages_for_openai_reasoning_model_instructions() {
		run(|host| async move {
			let params = capture(
				&host,
				&builtin_model("openai", "gpt-5.5"),
				with_system_prompt("Hi"),
				simple(None),
			)
			.await;
			assert_eq!(params["messages"][0]["role"], json!("developer"));
		});
	}

	#[test]
	fn stores_openrouter_kimi_k2_6_reasoning_replay_compat_in_built_in_metadata() {
		// `:free` variant delisted from the OpenRouter API; the generator override
		// matches any `moonshotai/kimi-k2.6*` variant that is listed.
		let model = builtin("openrouter", "moonshotai/kimi-k2.6");
		assert_eq!(compat_value(&model, "supportsDeveloperRole"), Some(json!(false)));
		assert_eq!(
			compat_value(&model, "requiresReasoningContentOnAssistantMessages"),
			Some(json!(true))
		);
	}

	#[test]
	fn stores_xiaomi_mimo_reasoning_replay_compat_in_built_in_metadata() {
		for provider in [
			"xiaomi",
			"xiaomi-token-plan-cn",
			"xiaomi-token-plan-ams",
			"xiaomi-token-plan-sgp",
		] {
			let model = builtin(provider, "mimo-v2.6-pro");
			assert_eq!(
				compat_value(&model, "requiresReasoningContentOnAssistantMessages"),
				Some(json!(true))
			);
			assert_eq!(compat_value(&model, "thinkingFormat"), Some(json!("deepseek")));
			assert_eq!(compat_value(&model, "maxTokensField"), None);
			assert_eq!(compat_value(&model, "supportsDeveloperRole"), None);
		}
	}

	#[test]
	fn stores_qwen_token_plan_reasoning_replay_compat_in_built_in_metadata() {
		for provider in ["qwen-token-plan", "qwen-token-plan-cn", "qwen-token-plan-individual"] {
			let model = builtin(provider, "qwen3.7-max");
			assert_eq!(compat_value(&model, "thinkingFormat"), Some(json!("qwen")));
			assert_eq!(
				compat_value(&model, "requiresReasoningContentOnAssistantMessages"),
				None
			);
			assert_eq!(compat_value(&model, "supportsDeveloperRole"), Some(json!(false)));
			assert_eq!(compat_value(&model, "supportsStore"), Some(json!(false)));
		}
	}

	#[test]
	fn replays_xiaomi_mimo_assistant_tool_calls_with_empty_reasoning_content_when_thinking_is_missing() {
		run(|host| async move {
			let assistant_message =
				tool_call_message("xiaomi", "mimo-v2.6-pro", vec![read_call()], StopReason::ToolUse);
			let params = capture(
				&host,
				&builtin("xiaomi", "mimo-v2.6-pro"),
				messages(vec![user("Read README.md"), assistant_message, read_result()]),
				simple(Some(ThinkingLevel::High)),
			)
			.await;
			let replayed_assistant = assistant_payload(&params);
			assert_eq!(replayed_assistant["role"], json!("assistant"));
			assert_eq!(replayed_assistant["reasoning_content"], json!(""));
			assert_eq!(params["thinking"], json!({ "type": "enabled" }));
			assert_eq!(params.get("reasoning_effort"), None);
		});
	}

	#[test]
	fn normalizes_opencode_go_reasoning_deltas_to_reasoning_content_for_replay() {
		run(|host| async move {
			let chunks = vec![json!({
				"id": "chatcmpl-opencode-go-reasoning",
				"choices": [{ "delta": { "reasoning": "think" }, "finish_reason": "stop" }],
			})];
			let (_, response) = respond(
				&host,
				chunks,
				&builtin_model("opencode-go", "kimi-k3"),
				messages(vec![user("Use reasoning.")]),
			)
			.await;
			assert_eq!(
				content_json(&response),
				json!([{ "type": "thinking", "thinking": "think", "thinkingSignature": "reasoning_content" }])
			);
		});
	}

	#[test]
	fn keeps_non_opencode_go_reasoning_deltas_on_the_original_reasoning_field() {
		run(|host| async move {
			let chunks = vec![json!({
				"id": "chatcmpl-reasoning",
				"choices": [{ "delta": { "reasoning": "think" }, "finish_reason": "stop" }],
			})];
			let (_, response) = respond(
				&host,
				chunks,
				&builtin_model("openai", "gpt-4o-mini"),
				messages(vec![user("Use reasoning.")]),
			)
			.await;
			assert_eq!(
				content_json(&response),
				json!([{ "type": "thinking", "thinking": "think", "thinkingSignature": "reasoning" }])
			);
		});
	}

	#[test]
	fn replays_opencode_go_reasoning_thinking_blocks_as_reasoning_content() {
		run(|_host| async move { replay_opencode_go_reasoning() });
	}

	fn replay_opencode_go_reasoning() {
		let model = builtin_model("opencode-go", "kimi-k3");
		let compat = ResolvedOpenAICompletionsCompat {
			supports_store: false,
			supports_developer_role: false,
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
			supports_thinking_token_budget: None,
			thinking_token_budget_field: None,
			supports_strict_mode: true,
			supports_openai_grammar_tools: false,
			supports_mid_convo_system_messages: None,
			supports_mid_convo_tool_additions: None,
			cache_control_format: None,
			send_session_affinity_headers: false,
			session_affinity_format: SessionAffinityFormat::Openai,
			supports_long_cache_retention: true,
			vllm_priority: None,
		};
		let messages = convert_messages(
			&model,
			&context(vec![tool_call_message(
				"opencode-go",
				"kimi-k3",
				vec![thinking("think", "reasoning"), read_call()],
				StopReason::Stop,
			)]),
			&compat,
			None,
		)
		.unwrap();

		assert_eq!(messages[0]["role"], json!("assistant"));
		assert_eq!(messages[0]["reasoning_content"], json!("think"));
		assert!(messages[0].get("reasoning").is_none());
	}

	#[test]
	fn sends_thinking_disabled_for_opencode_kimi_k2_6_when_thinking_is_off() {
		run(|host| async move {
			let params = capture_simple_params(&host, &builtin("opencode", "kimi-k2.6"), None).await;
			assert_eq!(params["thinking"], json!({ "type": "disabled" }));
			assert_eq!(params.get("reasoning_effort"), None);
		});
	}

	#[test]
	fn sends_thinking_enabled_for_opencode_kimi_k2_6_when_thinking_is_enabled() {
		run(|host| async move {
			let params =
				capture_simple_params(&host, &builtin("opencode", "kimi-k2.6"), Some(ThinkingLevel::High)).await;
			assert_eq!(params["thinking"], json!({ "type": "enabled" }));
			assert_eq!(params.get("reasoning_effort"), None);
		});
	}

	#[test]
	fn omits_disabled_thinking_for_moonshot_kimi_k2_7_code_models() {
		run(|host| async move {
			for model in [
				builtin("moonshotai", "kimi-k2.7-code"),
				builtin("moonshotai-cn", "kimi-k2.7-code"),
			] {
				let params = capture_simple_params(&host, &model, None).await;
				assert_eq!(params.get("thinking"), None);
				assert_eq!(params.get("reasoning_effort"), None);
			}
		});
	}

	#[test]
	fn keeps_disabled_thinking_for_moonshot_kimi_k2_6_when_thinking_is_off() {
		run(|host| async move {
			let params = capture_simple_params(&host, &builtin("moonshotai-cn", "kimi-k2.6"), None).await;
			assert_eq!(params["thinking"], json!({ "type": "disabled" }));
			assert_eq!(params.get("reasoning_effort"), None);
		});
	}

	async fn assert_sends_max_tokens(host: &TestHost, model: &Model) {
		let mut options = simple(None);
		options.base.max_tokens = Some(123.0);
		let params = capture(host, model, messages(vec![user("Hi")]), options).await;
		assert_eq!(params["max_tokens"], json!(123), "{}", model.id);
		assert_eq!(params.get("max_completion_tokens"), None, "{}", model.id);
	}

	#[test]
	fn sends_max_tokens_for_opencode_completions_models() {
		run(|host| async move {
			for model in [builtin("opencode-go", "kimi-k3"), builtin("opencode", "kimi-k2.6")] {
				assert_eq!(compat_value(&model, "maxTokensField"), Some(json!("max_tokens")));
				assert_sends_max_tokens(&host, &model).await;
			}
		});
	}

	#[test]
	fn sends_max_tokens_for_built_in_and_custom_deepseek_api_models() {
		run(|host| async move {
			let mut custom_model = local_openai_completions_model("custom-deepseek-model", "Custom DeepSeek Model");
			custom_model.base.provider = "custom-deepseek".into();
			custom_model.base.base_url = "https://api.deepseek.com".into();
			let mut custom_uppercase_model = custom_model.clone();
			custom_uppercase_model.base.id = "custom-uppercase-deepseek-model".into();
			custom_uppercase_model.base.name = "Custom Uppercase DeepSeek Model".into();
			custom_uppercase_model.base.base_url = "https://API.DeepSeek.COM".into();
			let native_models = [
				builtin("deepseek", "deepseek-flash"),
				builtin("deepseek", "deepseek-v4-pro"),
			];

			for model in &native_models {
				assert_eq!(compat_value(model, "maxTokensField"), Some(json!("max_tokens")));
			}

			for model in native_models.iter().chain([&custom_model, &custom_uppercase_model]) {
				assert_sends_max_tokens(&host, model).await;
			}
		});
	}

	#[test]
	fn sends_max_tokens_for_z_ai_completions_models() {
		run(|host| async move {
			for model in [builtin("zai", "glm-5-turbo"), builtin("zai", "glm-5.2")] {
				assert_eq!(compat_value(&model, "maxTokensField"), Some(json!("max_tokens")));
				assert_sends_max_tokens(&host, &model).await;
			}
		});
	}

	#[test]
	fn omits_reasoning_effort_for_opencode_grok_build() {
		run(|host| async move {
			let params =
				capture_simple_params(&host, &builtin("opencode", "grok-build-0.1"), Some(ThinkingLevel::High)).await;
			assert_eq!(params.get("reasoning_effort"), None);
		});
	}

	#[test]
	fn does_not_double_count_reasoning_tokens_in_completion_usage() {
		run(|host| async move {
			let chunks = vec![json!({
				"id": "chatcmpl-reasoning-usage",
				"choices": [{ "delta": {}, "finish_reason": "stop" }],
				"usage": {
					"prompt_tokens": 10,
					"completion_tokens": 33,
					"prompt_tokens_details": { "cached_tokens": 0 },
					"completion_tokens_details": { "reasoning_tokens": 21 },
				},
			})];
			let (_, response) = respond(
				&host,
				chunks,
				&builtin_model("openai", "gpt-4o-mini"),
				messages(vec![user("Use reasoning.")]),
			)
			.await;
			assert_eq!(response.usage.input, 10.0);
			assert_eq!(response.usage.output, 33.0);
			assert_eq!(response.usage.total_tokens, 43.0);
		});
	}

	fn cache_usage() -> Value {
		json!({
			"prompt_tokens": 100,
			"completion_tokens": 5,
			"prompt_tokens_details": { "cached_tokens": 50, "cache_write_tokens": 30 },
			"completion_tokens_details": { "reasoning_tokens": 0 },
		})
	}

	async fn assert_cache_usage(host: &TestHost, chunks: Vec<Value>) {
		let (_, response) = respond(
			host,
			chunks,
			&builtin_model("openai", "gpt-4o-mini"),
			messages(vec![user("Reply with exactly OK")]),
		)
		.await;
		// cached_tokens is documented as cache reads; cache_write_tokens is separate.
		assert_eq!(response.usage.input, 20.0);
		assert_eq!(response.usage.cache_read, 50.0);
		assert_eq!(response.usage.cache_write, 30.0);
		assert_eq!(response.usage.total_tokens, 105.0);
	}

	#[test]
	fn preserves_prompt_tokens_details_cache_read_write_fields_from_chunk_usage() {
		run(|host| async move {
			let chunks = vec![
				json!({ "id": "chatcmpl-cache-write", "choices": [{ "delta": { "content": "OK" }, "finish_reason": null }] }),
				json!({
					"id": "chatcmpl-cache-write",
					"choices": [{ "delta": {}, "finish_reason": "stop" }],
					"usage": cache_usage(),
				}),
			];
			assert_cache_usage(&host, chunks).await;
		});
	}

	#[test]
	fn preserves_prompt_tokens_details_cache_read_write_fields_from_choice_usage_fallback() {
		run(|host| async move {
			let chunks = vec![
				json!({
					"id": "chatcmpl-cache-write-choice",
					"choices": [{ "delta": { "content": "OK" }, "finish_reason": null }],
				}),
				json!({
					"id": "chatcmpl-cache-write-choice",
					"choices": [{ "delta": {}, "finish_reason": "stop", "usage": cache_usage() }],
				}),
			];
			assert_cache_usage(&host, chunks).await;
		});
	}

	#[test]
	fn uses_openrouter_reasoning_object_instead_of_reasoning_effort() {
		run(|host| async move {
			let params = capture_simple_params(
				&host,
				&builtin("openrouter", "deepseek/deepseek-r1-0528"),
				Some(ThinkingLevel::High),
			)
			.await;
			assert_eq!(params["reasoning"], json!({ "effort": "high" }));
			assert_eq!(params.get("reasoning_effort"), None);
		});
	}

	#[test]
	fn uses_configurable_chat_template_boolean_thinking_kwargs() {
		run(|host| async move {
			let mut model = local_openai_completions_model("deepseek-ai/DeepSeek-V3.1", "DeepSeek V3.1 via vLLM");
			model.compat = json!({
				"thinkingFormat": "chat-template",
				"supportsReasoningEffort": false,
				"chatTemplateKwargs": { "thinking": { "$var": "thinking.enabled" } },
			})
			.as_object()
			.cloned();

			for (reasoning, expected) in [(Some(ThinkingLevel::High), true), (None, false)] {
				let params = capture_simple_params(&host, &model, reasoning).await;
				assert_eq!(params["chat_template_kwargs"], json!({ "thinking": expected }));
				assert_eq!(params.get("thinking"), None);
				assert_eq!(params.get("reasoning_effort"), None);
			}
		});
	}

	#[test]
	fn uses_qwen_chat_template_thinking_kwargs() {
		run(|host| async move {
			let mut model = local_openai_completions_model("Qwen/Qwen3-Coder", "Qwen3 Coder via vLLM");
			model.compat = json!({ "thinkingFormat": "qwen-chat-template", "supportsReasoningEffort": false })
				.as_object()
				.cloned();

			for (reasoning, expected) in [(Some(ThinkingLevel::High), true), (None, false)] {
				let params = capture_simple_params(&host, &model, reasoning).await;
				assert_eq!(
					params["chat_template_kwargs"],
					json!({ "enable_thinking": expected, "preserve_thinking": true })
				);
				assert_eq!(params.get("reasoning_effort"), None);
			}
		});
	}

	#[test]
	fn uses_configurable_chat_template_effort_kwargs_with_static_kwargs() {
		run(|host| async move {
			let mut model = local_openai_completions_model("unsloth/gpt-oss-120b-GGUF", "GPT OSS via vLLM");
			model.thinking_level_map = Some(serde_json::from_value(json!({ "xhigh": "max" })).unwrap());
			model.compat = json!({
				"thinkingFormat": "chat-template",
				"supportsReasoningEffort": false,
				"chatTemplateKwargs": {
					"preserve_thinking": true,
					"reasoning_effort": { "$var": "thinking.effort", "omitWhenOff": true },
				},
			})
			.as_object()
			.cloned();

			let params = capture_simple_params(&host, &model, Some(ThinkingLevel::Xhigh)).await;
			assert_eq!(
				params["chat_template_kwargs"],
				json!({ "preserve_thinking": true, "reasoning_effort": "max" })
			);
			assert_eq!(params.get("reasoning_effort"), None);
		});
	}

	#[test]
	fn uses_ant_ling_compatibility_metadata() {
		run(|host| async move {
			let model = builtin("ant-ling", "Ring-2.6-1T");
			for (key, value) in [
				("supportsStore", json!(false)),
				("supportsDeveloperRole", json!(false)),
				("supportsReasoningEffort", json!(false)),
				("maxTokensField", json!("max_tokens")),
				("thinkingFormat", json!("ant-ling")),
				("supportsLongCacheRetention", json!(false)),
			] {
				assert_eq!(compat_value(&model, key), Some(value), "{key}");
			}
			assert_eq!(compat_value(&model, "supportsStrictMode"), Some(json!(true)));
			assert_eq!(
				compat_value(&model, "requiresReasoningContentOnAssistantMessages"),
				None
			);

			let mut options = simple(Some(ThinkingLevel::High));
			options.base.max_tokens = Some(123.0);
			options.base.cache_retention = Some(pi_ai::types::CacheRetention::Long);
			options.base.session_id = Some("ant-ling-session".into());
			let params = capture(&host, &model, with_system_prompt("Hi"), options).await;

			assert_eq!(params["max_tokens"], json!(123));
			assert_eq!(params.get("max_completion_tokens"), None);
			assert_eq!(params["messages"][0]["role"], json!("system"));
			assert_eq!(params["reasoning"], json!({ "effort": "high" }));
			assert_eq!(params.get("reasoning_effort"), None);
			assert_eq!(params.get("store"), None);
			assert_eq!(params.get("prompt_cache_key"), None);
			assert_eq!(params.get("prompt_cache_retention"), None);
		});
	}

	#[test]
	fn omits_ant_ling_reasoning_for_unmapped_direct_reasoning_efforts_and_non_reasoning_models() {
		run(|host| async move {
			serve_chunks(&host, vec![default_chunk()]);
			let ring = builtin("ant-ling", "Ring-2.6-1T");
			let (on_payload, payload) = capture_payload(None);
			let mut options = ProviderStreamOptions::default();
			options.base.api_key = Some("test".into());
			options.base.on_payload = Some(on_payload);
			options.extra.insert("reasoningEffort".into(), json!("medium"));
			stream(&ring, &messages(vec![user("Hi")]), Some(options))
				.unwrap()
				.result()
				.await
				.unwrap();
			let params = payload.borrow().clone().unwrap_or_else(|| last_body(&host));
			assert!(!params.as_object().unwrap().contains_key("reasoning"));

			let ling = builtin("ant-ling", "Ling-2.6-flash");
			let params = capture_simple_params(&host, &ling, Some(ThinkingLevel::High)).await;
			assert!(!params.as_object().unwrap().contains_key("reasoning"));
		});
	}
}
