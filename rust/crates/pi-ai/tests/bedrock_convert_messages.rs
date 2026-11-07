//! Port of `packages/ai/test/bedrock-convert-messages.test.ts`.
//!
//! Differences from the TS:
//! - The TS mocks `@aws-sdk/client-bedrock-runtime`. `capturePayload` reads the command input through `onPayload`
//!   with an already aborted signal, so nothing is sent. The streamed tool call is answered by `bedrock_support::serve`
//!   with `AWS_BEARER_TOKEN_BEDROCK` set so the client can authenticate.
//! - The content block enums are closed (`TextContent | ImageContent`, `TextContent | ThinkingContent | ToolCall`), so a
//!   `{ type: "unknown" }` block cannot be built: deserializing it into a `Context` fails. "skips unknown user content
//!   blocks instead of throwing" and "skips unknown assistant content blocks instead of throwing" are left out. "replaces
//!   user messages with only unknown content blocks with a placeholder" and "skips assistant messages with only unknown
//!   content blocks" are adapted to the content those messages are left with once the unknown blocks are skipped: an
//!   empty list.
//! - A Rust string cannot hold an unpaired surrogate (`String.fromCharCode(0xd83d)`), so "replaces user content emptied
//!   by surrogate sanitization with a placeholder" and "skips assistant text blocks emptied by surrogate sanitization"
//!   are left out.
//! - `expect.objectContaining({ format: "png" })` checks the `format` member of the image block.
//! - "removes empty property names only from replayed Bedrock input" checks that the context passed to `stream` still
//!   holds the empty property name, where the TS checks the arguments object it put in the context.

mod bedrock_support;

use std::cell::RefCell;
use std::rc::Rc;

use bedrock_support::{collect, event, model, serve, stream_response};
use pi_ai::api::bedrock_converse_stream::{BedrockOptions, stream};
use pi_ai::types::{AssistantContent, CacheRetention, Message, Model, StreamOptions, TranscriptContext};
use pi_host::AbortController;
use pi_host::testing::run;
use serde_json::{Value, json};

fn base_model() -> Model {
	model(json!({
		"id": "us.anthropic.claude-sonnet-4-5-20250929-v1:0",
		"name": "Claude Sonnet 4.5 (US)",
		"api": "bedrock-converse-stream",
		"provider": "amazon-bedrock",
		"baseUrl": "https://bedrock-runtime.us-east-1.amazonaws.com",
		"reasoning": true,
		"input": ["text", "image"],
		"cost": { "input": 3, "output": 15, "cacheRead": 0.3, "cacheWrite": 3.75 },
		"contextWindow": 200000,
		"maxTokens": 64000,
		"compat": { "supportsStrictMode": true },
	}))
}

fn nova_model() -> Model {
	model(json!({
		"id": "amazon.nova-lite-v1:0",
		"name": "Nova Lite",
		"api": "bedrock-converse-stream",
		"provider": "amazon-bedrock",
		"baseUrl": "https://bedrock-runtime.us-east-1.amazonaws.com",
		"reasoning": false,
		"input": ["text", "image"],
		"cost": { "input": 3, "output": 15, "cacheRead": 0.3, "cacheWrite": 3.75 },
		"contextWindow": 200000,
		"maxTokens": 64000,
	}))
}

fn empty_usage() -> Value {
	json!({
		"input": 0,
		"output": 0,
		"cacheRead": 0,
		"cacheWrite": 0,
		"totalTokens": 0,
		"cost": { "input": 0, "output": 0, "cacheRead": 0, "cacheWrite": 0, "total": 0 },
	})
}

/// An assistant message literal of `baseModel` with `content` and `stopReason`.
fn assistant(content: Value, stop_reason: &str) -> Value {
	json!({
		"role": "assistant",
		"content": content,
		"api": "bedrock-converse-stream",
		"provider": "amazon-bedrock",
		"model": base_model().id,
		"usage": empty_usage(),
		"stopReason": stop_reason,
		"timestamp": 1,
	})
}

/// `{ cacheRetention: "none" }`.
fn no_cache() -> BedrockOptions {
	BedrockOptions {
		base: StreamOptions {
			cache_retention: Some(CacheRetention::None),
			..StreamOptions::default()
		},
		..BedrockOptions::default()
	}
}

/// `capturePayload(context, model)`.
async fn capture_payload(context: &TranscriptContext, model: &Model) -> Option<Value> {
	let captured = Rc::new(RefCell::new(None));
	let recorded = captured.clone();
	let controller = AbortController::new();
	controller.abort();
	let mut options = no_cache();
	options.signal = Some(controller.signal());
	options.on_payload = Some(Rc::new(move |payload: Value, _: &Model| {
		*recorded.borrow_mut() = Some(payload.clone());
		Box::pin(async move { Ok(Some(payload)) })
	}));
	collect(stream(model, context, Some(options))).await;
	captured.take()
}

/// `capturePayload({ messages })` with the base model.
async fn capture_messages(messages: Value) -> Value {
	let context = bedrock_support::context(json!({ "messages": messages }));
	capture_payload(&context, &base_model())
		.await
		.expect("a captured payload")
}

mod bedrock_constrained_sampling {
	use super::*;

	fn context(strict: &str) -> TranscriptContext {
		bedrock_support::context(json!({
			"messages": [{ "role": "user", "content": "Use the tool", "timestamp": 1 }],
			"tools": [{
				"name": "lookup",
				"description": "Look up a value",
				"parameters": {
					"type": "object",
					"properties": { "value": { "type": "string" } },
					"required": ["value"],
				},
				"constrainedSampling": { "type": "json_schema", "strict": strict },
			}],
		}))
	}

	#[test]
	fn gates_native_strict_tool_use_by_model_capability() {
		run(|_| async {
			let payload = capture_payload(&context("require"), &base_model()).await.unwrap();
			assert_eq!(payload["toolConfig"]["tools"][0]["toolSpec"]["strict"], json!(true));

			let nova_payload = capture_payload(&context("prefer"), &nova_model()).await.unwrap();
			assert_eq!(nova_payload["toolConfig"]["tools"][0]["toolSpec"].get("strict"), None);
		});
	}
}

mod bedrock_tool_arguments {
	use super::*;

	#[test]
	fn preserves_empty_property_names_in_streamed_tool_arguments() {
		run(|host| async move {
			host.set_env("AWS_BEARER_TOKEN_BEDROCK", "tok");
			serve(
				&host,
				vec![stream_response(
					&[
						event("messageStart", json!({ "role": "assistant" })),
						event(
							"contentBlockStart",
							json!({
								"contentBlockIndex": 0,
								"start": { "toolUse": { "toolUseId": "tool-1", "name": "edit" } },
							}),
						),
						event(
							"contentBlockDelta",
							json!({
								"contentBlockIndex": 0,
								"delta": {
									"toolUse": {
										"input": r#"{"path":"/workspace/foobar/file.js","edits":[{"oldText":"first","newText":"updated first"},{"oldText":"second","newText":"updated second","":""}]}"#,
									},
								},
							}),
						),
						event("contentBlockStop", json!({ "contentBlockIndex": 0 })),
						event("messageStop", json!({ "stopReason": "tool_use" })),
					],
					&[],
				)],
			);
			let context = bedrock_support::context(json!({
				"messages": [{ "role": "user", "content": "Use the tool", "timestamp": 1 }],
			}));
			let message = stream(&base_model(), &context, Some(no_cache()))
				.result()
				.await
				.unwrap();

			assert_eq!(
				serde_json::to_value(&message.content[0]).unwrap(),
				json!({
					"type": "toolCall",
					"id": "tool-1",
					"name": "edit",
					"arguments": {
						"path": "/workspace/foobar/file.js",
						"edits": [
							{ "oldText": "first", "newText": "updated first" },
							{ "oldText": "second", "newText": "updated second", "": "" },
						],
					},
				})
			);
		});
	}
}

mod bedrock_convert_messages_skips_unknown_content_types {
	use super::*;

	#[test]
	fn replaces_user_messages_with_only_unknown_content_blocks_with_a_placeholder() {
		run(|_| async {
			let payload = capture_messages(json!([{ "role": "user", "content": [], "timestamp": 1 }])).await;
			assert_eq!(payload["messages"].as_array().unwrap().len(), 1);
			assert_eq!(payload["messages"][0]["content"], json!([{ "text": "<empty>" }]));
		});
	}

	#[test]
	fn replaces_blank_user_string_content_with_a_placeholder() {
		run(|_| async {
			let payload = capture_messages(json!([{ "role": "user", "content": "   ", "timestamp": 1 }])).await;
			assert_eq!(payload["messages"].as_array().unwrap().len(), 1);
			assert_eq!(payload["messages"][0]["content"], json!([{ "text": "<empty>" }]));
		});
	}

	#[test]
	fn filters_blank_user_text_blocks_when_other_content_remains() {
		run(|_| async {
			let payload = capture_messages(json!([{
				"role": "user",
				"content": [
					{ "type": "text", "text": "" },
					{ "type": "text", "text": "hello" },
				],
				"timestamp": 1,
			}]))
			.await;
			assert_eq!(payload["messages"].as_array().unwrap().len(), 1);
			assert_eq!(payload["messages"][0]["content"], json!([{ "text": "hello" }]));
		});
	}

	#[test]
	fn replaces_blank_tool_result_content_with_a_placeholder() {
		run(|_| async {
			let payload = capture_messages(json!([{
				"role": "toolResult",
				"toolCallId": "tool-1",
				"toolName": "tool",
				"content": [{ "type": "text", "text": "" }],
				"isError": false,
				"timestamp": 1,
			}]))
			.await;
			assert_eq!(payload["messages"].as_array().unwrap().len(), 1);
			assert_eq!(
				payload["messages"][0]["content"][0]["toolResult"]["content"],
				json!([{ "text": "<empty>" }])
			);
		});
	}

	// Regression for #10794: Claude rejects error tool results that contain images.
	#[test]
	fn moves_images_of_error_tool_results_after_the_tool_results() {
		run(|_| async {
			let payload = capture_messages(json!([
				assistant(
					json!([
						{ "type": "toolCall", "id": "call-1", "name": "codemode", "arguments": {} },
						{ "type": "toolCall", "id": "call-2", "name": "read", "arguments": {} },
					]),
					"toolUse",
				),
				{
					"role": "toolResult",
					"toolCallId": "call-1",
					"toolName": "codemode",
					"content": [
						{ "type": "text", "text": "Script failed" },
						{ "type": "image", "data": "AAAA", "mimeType": "image/png" },
						{ "type": "text", "text": "Script error" },
					],
					"isError": true,
					"timestamp": 1,
				},
				{
					"role": "toolResult",
					"toolCallId": "call-2",
					"toolName": "read",
					"content": [{ "type": "image", "data": "BBBB", "mimeType": "image/png" }],
					"isError": false,
					"timestamp": 1,
				},
			]))
			.await;
			let content = payload["messages"][1]["content"].as_array().unwrap();
			assert_eq!(content.len(), 4);
			assert_eq!(
				content[0]["toolResult"],
				json!({
					"toolUseId": "call-1",
					"content": [
						{ "text": "Script failed" },
						{ "text": "(image attached after the tool results)" },
						{ "text": "Script error" },
					],
					"status": "error",
				})
			);
			assert_eq!(content[1]["toolResult"]["status"], "success");
			let result_content = content[1]["toolResult"]["content"].as_array().unwrap();
			assert_eq!(result_content.len(), 1);
			assert_image_block(&result_content[0]);
			assert_eq!(
				content[2],
				json!({ "text": "Images from the error result of tool call call-1 (codemode):" })
			);
			assert_image_block(&content[3]);
		});
	}

	#[test]
	fn keeps_images_in_error_tool_results_for_non_claude_models() {
		run(|_| async {
			let context = bedrock_support::context(json!({
				"messages": [{
					"role": "toolResult",
					"toolCallId": "call-1",
					"toolName": "codemode",
					"content": [
						{ "type": "text", "text": "Script failed" },
						{ "type": "image", "data": "AAAA", "mimeType": "image/png" },
					],
					"isError": true,
					"timestamp": 1,
				}],
			}));
			let payload = capture_payload(&context, &nova_model()).await.unwrap();
			let content = payload["messages"][0]["content"].as_array().unwrap();
			assert_eq!(content.len(), 1);
			assert_eq!(
				content[0].as_object().unwrap().keys().collect::<Vec<_>>(),
				["toolResult"]
			);
			let tool_result = &content[0]["toolResult"];
			assert_eq!(
				tool_result.as_object().unwrap().keys().collect::<Vec<_>>(),
				["toolUseId", "content", "status"]
			);
			assert_eq!(tool_result["toolUseId"], "call-1");
			assert_eq!(tool_result["status"], "error");
			let result_content = tool_result["content"].as_array().unwrap();
			assert_eq!(result_content.len(), 2);
			assert_eq!(result_content[0], json!({ "text": "Script failed" }));
			assert_image_block(&result_content[1]);
		});
	}

	#[test]
	fn skips_assistant_messages_with_only_unknown_content_blocks() {
		run(|_| async {
			let payload = capture_messages(json!([assistant(json!([]), "stop")])).await;
			assert_eq!(payload["messages"].as_array().unwrap().len(), 0);
		});
	}

	#[test]
	fn removes_empty_property_names_only_from_replayed_bedrock_input() {
		run(|_| async {
			let tool_arguments = json!({
				"path": "/workspace/foobar/file.js",
				"edits": [
					{ "oldText": "first", "newText": "updated first" },
					{ "oldText": "second", "newText": "updated second", "": "" },
				],
			});
			let context = bedrock_support::context(json!({
				"messages": [
					assistant(
						json!([{ "type": "toolCall", "id": "tool-1", "name": "edit", "arguments": tool_arguments }]),
						"toolUse",
					),
					{
						"role": "toolResult",
						"toolCallId": "tool-1",
						"toolName": "edit",
						"content": [{ "type": "text", "text": "done" }],
						"isError": false,
						"timestamp": 1,
					},
					{ "role": "user", "content": "Continue", "timestamp": 1 },
				],
			}));

			let payload = capture_payload(&context, &base_model()).await.unwrap();
			assert_eq!(
				payload["messages"][0]["content"][0]["toolUse"]["input"],
				json!({
					"path": "/workspace/foobar/file.js",
					"edits": [
						{ "oldText": "first", "newText": "updated first" },
						{ "oldText": "second", "newText": "updated second" },
					],
				})
			);
			let Message::Assistant(message) = &context.messages[0] else {
				panic!("expected the assistant message first");
			};
			let AssistantContent::ToolCall(tool_call) = &message.content[0] else {
				panic!("expected a tool call");
			};
			assert_eq!(
				tool_call.arguments["edits"][1],
				json!({ "oldText": "second", "newText": "updated second", "": "" })
			);
		});
	}

	/// `{ image: expect.objectContaining({ format: "png" }) }`.
	fn assert_image_block(block: &Value) {
		assert_eq!(block.as_object().unwrap().keys().collect::<Vec<_>>(), ["image"]);
		assert_eq!(block["image"]["format"], "png");
	}
}
