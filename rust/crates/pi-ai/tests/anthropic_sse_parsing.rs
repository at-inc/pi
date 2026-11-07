//! Port of `packages/ai/test/anthropic-sse-parsing.test.ts`.

mod anthropic_support;

use std::cell::RefCell;
use std::rc::Rc;

use anthropic_support::{
	FakeClient, SseEvent, context, event, minimal_events, model, raw_event, sse_response, user_context,
};
use pi_ai::api::anthropic_messages::{AnthropicOptions, stream};
use pi_ai::api::transform_messages::transform_messages;
use pi_ai::types::{AssistantContent, Message, Model, StopReason, StreamOptions};
use pi_host::testing::run;
use serde_json::{Value, json};

/// `createResponseModelSseResponse(model, contentBlock)`.
fn response_model_events(model: &str, content_block: Value) -> Vec<SseEvent> {
	vec![
		event(
			"message_start",
			json!({
				"type": "message_start",
				"message": { "id": "msg_response_model", "model": model, "usage": { "input_tokens": 100, "output_tokens": 0 } },
			}),
		),
		event(
			"content_block_start",
			json!({ "type": "content_block_start", "index": 0, "content_block": content_block }),
		),
		event(
			"content_block_stop",
			json!({ "type": "content_block_stop", "index": 0 }),
		),
		event(
			"message_delta",
			json!({
				"type": "message_delta",
				"delta": { "stop_reason": "end_turn" },
				"usage": { "input_tokens": 100, "output_tokens": 20 },
			}),
		),
		event("message_stop", json!({ "type": "message_stop" })),
	]
}

fn client_options(client: Rc<FakeClient>) -> AnthropicOptions {
	AnthropicOptions {
		client: Some(client),
		..AnthropicOptions::default()
	}
}

fn content_json(content: &[AssistantContent]) -> Value {
	serde_json::to_value(content).unwrap()
}

mod anthropic_raw_sse_parsing {
	use super::*;

	#[test]
	fn forwards_parsed_provider_stream_events_in_order() {
		run(|_| async {
			let model = model("anthropic", "claude-haiku-4-5");
			let provider_events: Rc<RefCell<Vec<Value>>> = Rc::default();
			let event_models: Rc<RefCell<Vec<Model>>> = Rc::default();
			let (events, models) = (provider_events.clone(), event_models.clone());
			let options = AnthropicOptions {
				base: StreamOptions {
					on_provider_stream_event: Some(Rc::new(move |event, event_model| {
						events.borrow_mut().push(event.clone());
						models.borrow_mut().push(event_model.clone());
						Box::pin(async { Ok(()) })
					})),
					..StreamOptions::default()
				},
				..client_options(FakeClient::new(sse_response(&minimal_events())))
			};
			let result = stream(&model, &user_context("Hello"), Some(options))
				.result()
				.await
				.unwrap();

			assert_eq!(result.stop_reason, StopReason::Stop);
			let types: Vec<String> = provider_events
				.borrow()
				.iter()
				.map(|event| event["type"].as_str().unwrap().to_string())
				.collect();
			assert_eq!(
				types,
				[
					"message_start",
					"content_block_start",
					"content_block_delta",
					"content_block_stop",
					"message_delta",
					"message_stop",
				]
			);
			assert_eq!(*event_models.borrow(), vec![model.clone(); 6]);
		});
	}

	// Regression test for #9980.
	#[test]
	fn uses_the_total_reported_by_open_routers_anthropic_messages_endpoint() {
		run(|_| async {
			let mut events = minimal_events();
			let mut delta: Value = serde_json::from_str(&events[4].1).unwrap();
			// These fields were captured from a production OpenRouter stream even though its
			// MessagesDeltaEvent OpenAPI schema and the Anthropic SDK omit them.
			let usage = delta["usage"].as_object_mut().unwrap();
			usage.insert("cost".into(), json!(0.004));
			usage.insert("is_byok".into(), json!(false));
			usage.insert("cost_details".into(), json!({ "upstream_inference_cost": 0.003 }));
			events[4].1 = pi_host::js::json_stringify(&delta);

			let mut model = model("openrouter", "anthropic/claude-haiku-4.5");
			model.provider = "openrouter-work".into();
			let result = stream(
				&model,
				&user_context("Hello"),
				Some(client_options(FakeClient::new(sse_response(&events)))),
			)
			.result()
			.await
			.unwrap();

			assert_eq!(result.usage.cost.total, 0.004);
			assert_eq!(result.usage.cost.total_from_provider, Some(true));
			let cost = &result.usage.cost;
			assert_ne!(
				cost.input + cost.output + cost.cache_read + cost.cache_write,
				cost.total
			);
		});
	}

	#[test]
	fn keeps_signed_thinking_replayable_when_a_proxy_relabels_the_model() {
		// Regression test for earendil-works/pi#9188.
		run(|_| async {
			let model = model("anthropic", "claude-opus-5");
			let response_model = "kimi-for-coding";
			let initial_context = user_context("Hello");
			let first = stream(
				&model,
				&initial_context,
				Some(client_options(FakeClient::new(sse_response(&response_model_events(
					response_model,
					json!({ "type": "thinking", "thinking": "reasoning", "signature": "signature" }),
				))))),
			)
			.result()
			.await
			.unwrap();

			assert_eq!(first.model, model.id);
			assert_eq!(first.response_model.as_deref(), Some(response_model));

			let mut messages = initial_context.messages.clone();
			messages.push(Message::Assistant(first));
			let transformed = transform_messages(&messages, &model, None);
			let replayed_assistant = transformed
				.iter()
				.find_map(|message| match message {
					Message::Assistant(message) => Some(message),
					_ => None,
				})
				.unwrap();
			assert_eq!(
				content_json(&replayed_assistant.content),
				json!([{ "type": "thinking", "thinking": "reasoning", "thinkingSignature": "signature" }])
			);
		});
	}

	#[test]
	fn uses_a_returned_fallback_model_for_cost_attribution() {
		run(|_| async {
			let fallback_model = "fallback-model";
			let mut model = model("anthropic", "claude-opus-5");
			model.compat = Some(
				json!({
					"allowedFallbackModels": [{
						"provider": "anthropic",
						"model": fallback_model,
						"cost": { "input": 3, "output": 5, "cacheRead": 0, "cacheWrite": 0 },
					}],
				})
				.as_object()
				.unwrap()
				.clone(),
			);
			let result = stream(
				&model,
				&user_context("Hello"),
				Some(client_options(FakeClient::new(sse_response(&response_model_events(
					fallback_model,
					json!({ "type": "text", "text": "done" }),
				))))),
			)
			.result()
			.await
			.unwrap();

			assert_eq!(result.model, model.id);
			assert_eq!(result.response_model.as_deref(), Some(fallback_model));
			assert!((result.usage.cost.input - 0.0003).abs() < 1e-10);
			assert!((result.usage.cost.output - 0.0001).abs() < 1e-10);
		});
	}

	#[test]
	fn fails_safely_when_anthropic_falls_back_after_output_begins() {
		run(|_| async {
			let model = model("anthropic", "claude-opus-5");
			let response = sse_response(&[
				event(
					"message_start",
					json!({
						"type": "message_start",
						"message": { "id": "msg_fallback", "model": "claude-opus-5", "usage": { "input_tokens": 1, "output_tokens": 0 } },
					}),
				),
				event(
					"content_block_start",
					json!({ "type": "content_block_start", "index": 0, "content_block": { "type": "text", "text": "partial" } }),
				),
				event(
					"content_block_stop",
					json!({ "type": "content_block_stop", "index": 0 }),
				),
				event(
					"content_block_start",
					json!({
						"type": "content_block_start",
						"index": 1,
						"content_block": { "type": "fallback", "from": { "model": "claude-opus-5" }, "to": { "model": "claude-opus-4-8" } },
					}),
				),
			]);

			let result = stream(
				&model,
				&user_context("Hello"),
				Some(client_options(FakeClient::new(response))),
			)
			.result()
			.await
			.unwrap();

			assert_eq!(result.stop_reason, StopReason::Error);
			assert!(
				result
					.error_message
					.unwrap()
					.contains("unsupported mid-output model fallback")
			);
		});
	}

	#[test]
	fn forces_streaming_after_an_on_payload_replacement() {
		run(|_| async {
			let client = FakeClient::with(|_, _| Ok(sse_response(&minimal_events())));
			let options = AnthropicOptions {
				base: StreamOptions {
					base: pi_ai::types::ProviderRequestOptions {
						on_payload: Some(Rc::new(|payload, _| {
							let mut payload = payload.as_object().unwrap().clone();
							payload.insert("stream".into(), Value::Bool(false));
							Box::pin(async move { Ok(Some(Value::Object(payload))) })
						})),
						..Default::default()
					},
					..StreamOptions::default()
				},
				..client_options(client.clone())
			};

			stream(
				&model("anthropic", "claude-fable-5-1"),
				&user_context("Hello"),
				Some(options),
			)
			.result()
			.await
			.unwrap();

			assert_eq!(client.last_params()["stream"], json!(true));
		});
	}

	#[test]
	fn omits_the_interleaved_thinking_beta_when_thinking_is_disabled() {
		run(|_| async {
			let client = FakeClient::with(|_, _| Ok(sse_response(&minimal_events())));
			stream(
				&model("openrouter", "anthropic/claude-haiku-4.5"),
				&user_context("Hello"),
				Some(AnthropicOptions {
					thinking_enabled: Some(false),
					..client_options(client.clone())
				}),
			)
			.result()
			.await
			.unwrap();

			let betas = client.last_params().get("betas").cloned().unwrap_or(json!([]));
			assert!(
				!betas
					.as_array()
					.unwrap()
					.contains(&json!("interleaved-thinking-2025-05-14"))
			);
		});
	}

	#[test]
	fn passes_managed_beta_features_to_injected_clients() {
		run(|_| async {
			let client = FakeClient::with(|_, _| Ok(sse_response(&minimal_events())));
			let result = stream(
				&model("anthropic", "claude-fable-5-1"),
				&user_context("Hello"),
				Some(client_options(client.clone())),
			)
			.result()
			.await
			.unwrap();

			assert_eq!(result.stop_reason, StopReason::Stop);
			let betas = client.last_params()["betas"].clone();
			let betas = betas.as_array().unwrap();
			assert!(betas.contains(&json!("mid-conversation-output-config-2026-07-01")));
			assert!(betas.contains(&json!("thinking-binding-controls-2026-08-01")));
		});
	}

	#[test]
	fn uses_the_serving_model_input_transformations_from_the_final_stream_event() {
		run(|_| async {
			let mut events = minimal_events();
			events[0].1 = pi_host::js::json_stringify(&json!({
				"type": "message_start",
				"message": {
					"id": "msg_transformations",
					"model": "claude-fable-5-1",
					"usage": { "input_tokens": 12, "output_tokens": 0 },
					"input_transformations": [
						{ "type": "thinking_dropped", "path": "messages.1.content.0", "reason": "prefix_binding_mismatch" },
					],
				},
			}));
			let mut delta: Value = serde_json::from_str(&events[4].1).unwrap();
			delta["input_transformations"] = json!([{ "type": "thinking_dropped", "path": "messages.3.content.0", "reason": "model_binding_mismatch" }]);
			events[4].1 = pi_host::js::json_stringify(&delta);

			let result = stream(
				&model("anthropic", "claude-fable-5-1"),
				&user_context("Hello"),
				Some(client_options(FakeClient::new(sse_response(&events)))),
			)
			.result()
			.await
			.unwrap();

			let diagnostics = serde_json::to_value(result.diagnostics.unwrap()).unwrap();
			assert!(diagnostics[0]["timestamp"].is_number());
			let mut diagnostics = diagnostics;
			diagnostics[0].as_object_mut().unwrap().remove("timestamp");
			assert_eq!(
				diagnostics,
				json!([{
					"type": "anthropic_input_transformations",
					"details": {
						"transformations": [
							{ "type": "thinking_dropped", "path": "messages.3.content.0", "reason": "model_binding_mismatch" },
						],
					},
				}])
			);
		});
	}

	#[test]
	fn repairs_malformed_sse_json_and_malformed_streamed_tool_json() {
		run(|_| async {
			let model = model("anthropic", "claude-haiku-4-5");
			let context = context(json!({
				"messages": [{ "role": "user", "content": "Use the edit tool.", "timestamp": 1 }],
				"tools": [{
					"name": "edit",
					"description": "Edit a file.",
					"parameters": {
						"type": "object",
						"properties": { "path": { "type": "string" }, "text": { "type": "string" } },
						"required": ["path", "text"],
					},
				}],
			}));

			let malformed_tool_json_delta = concat!(
				r#"{"type":"content_block_delta","index":0,"delta":{"type":"input_json_delta","partial_json":"{\"path\":\"A\H\",\"text\":\"col1"#,
				"\t",
				r#"col2\"}"}}"#
			);

			let response = sse_response(&[
				event(
					"message_start",
					json!({
						"type": "message_start",
						"message": {
							"id": "msg_test",
							"usage": { "input_tokens": 12, "output_tokens": 0, "cache_read_input_tokens": 0, "cache_creation_input_tokens": 0 },
						},
					}),
				),
				event(
					"content_block_start",
					json!({
						"type": "content_block_start",
						"index": 0,
						"content_block": { "type": "tool_use", "id": "toolu_test", "name": "edit", "input": {} },
					}),
				),
				raw_event("content_block_delta", malformed_tool_json_delta),
				event(
					"content_block_stop",
					json!({ "type": "content_block_stop", "index": 0 }),
				),
				event(
					"message_delta",
					json!({
						"type": "message_delta",
						"delta": { "stop_reason": "tool_use" },
						"usage": { "input_tokens": 12, "output_tokens": 5, "cache_read_input_tokens": 0, "cache_creation_input_tokens": 0 },
					}),
				),
				event("message_stop", json!({ "type": "message_stop" })),
			]);

			let result = stream(&model, &context, Some(client_options(FakeClient::new(response))))
				.result()
				.await
				.unwrap();

			assert_eq!(result.stop_reason, StopReason::ToolUse);
			assert_eq!(result.error_message, None);

			let tool_call = result
				.content
				.iter()
				.find_map(|block| match block {
					AssistantContent::ToolCall(tool_call) => Some(tool_call),
					_ => None,
				})
				.expect("a tool call");
			assert_eq!(
				Value::Object(tool_call.arguments.clone()),
				json!({ "path": "A\\H", "text": "col1\tcol2" })
			);
		});
	}

	#[test]
	fn preserves_content_from_content_block_start_events() {
		run(|_| async {
			let model = model("anthropic", "claude-haiku-4-5");
			let usage = json!({ "input_tokens": 12, "output_tokens": 0, "cache_read_input_tokens": 0, "cache_creation_input_tokens": 0 });
			let response = sse_response(&[
				event(
					"message_start",
					json!({ "type": "message_start", "message": { "id": "msg_initial_content", "usage": usage } }),
				),
				event(
					"content_block_start",
					json!({ "type": "content_block_start", "index": 0, "content_block": { "type": "text", "text": "Initial text" } }),
				),
				event(
					"content_block_delta",
					json!({ "type": "content_block_delta", "index": 0, "delta": { "type": "text_delta", "text": " plus delta" } }),
				),
				event(
					"content_block_stop",
					json!({ "type": "content_block_stop", "index": 0 }),
				),
				event(
					"content_block_start",
					json!({
						"type": "content_block_start",
						"index": 1,
						"content_block": { "type": "thinking", "thinking": "Initial thinking", "signature": "initial signature" },
					}),
				),
				event(
					"content_block_delta",
					json!({ "type": "content_block_delta", "index": 1, "delta": { "type": "thinking_delta", "thinking": " plus delta" } }),
				),
				event(
					"content_block_delta",
					json!({ "type": "content_block_delta", "index": 1, "delta": { "type": "signature_delta", "signature": " plus delta" } }),
				),
				event(
					"content_block_stop",
					json!({ "type": "content_block_stop", "index": 1 }),
				),
				event(
					"message_delta",
					json!({
						"type": "message_delta",
						"delta": { "stop_reason": "end_turn" },
						"usage": { "input_tokens": 12, "output_tokens": 5, "cache_read_input_tokens": 0, "cache_creation_input_tokens": 0 },
					}),
				),
				event("message_stop", json!({ "type": "message_stop" })),
			]);

			let result = stream(
				&model,
				&user_context("Say hello."),
				Some(client_options(FakeClient::new(response))),
			)
			.result()
			.await
			.unwrap();

			assert_eq!(
				content_json(&result.content),
				json!([
					{ "type": "text", "text": "Initial text plus delta" },
					{
						"type": "thinking",
						"thinking": "Initial thinking plus delta",
						"thinkingSignature": "initial signature plus delta",
					},
				])
			);
		});
	}

	/// The stream of a `message_start`, one `message_delta` with `delta`, and `message_stop`.
	fn stop_events(id: &str, input_tokens: u64, delta: Value) -> Vec<SseEvent> {
		let usage = json!({
			"input_tokens": input_tokens,
			"output_tokens": 0,
			"cache_read_input_tokens": 0,
			"cache_creation_input_tokens": 0,
		});
		vec![
			event(
				"message_start",
				json!({ "type": "message_start", "message": { "id": id, "usage": usage } }),
			),
			event(
				"message_delta",
				json!({ "type": "message_delta", "delta": delta, "usage": usage }),
			),
			event("message_stop", json!({ "type": "message_stop" })),
		]
	}

	#[test]
	fn preserves_refusal_stop_details_from_message_delta() {
		run(|_| async {
			let model = model("anthropic", "claude-fable-5");
			let explanation = "This request triggered restrictions on violative cyber content and was blocked under Anthropic's Usage Policy. To learn more, provide feedback, or request an exemption based on how you use Claude, visit our help center: https://support.claude.com/en/articles/14604842-real-time-cyber-safeguards-on-claude.";
			let response = sse_response(&stop_events(
				"msg_01XFUDYJgAACzvnptvVoYEL",
				412,
				json!({
					"stop_reason": "refusal",
					"stop_details": { "type": "refusal", "category": "cyber", "explanation": explanation },
				}),
			));

			let result = stream(
				&model,
				&user_context("blocked request"),
				Some(client_options(FakeClient::new(response))),
			)
			.result()
			.await
			.unwrap();

			assert_eq!(result.stop_reason, StopReason::Error);
			assert_eq!(result.raw_stop_reason.as_deref(), Some("refusal"));
			assert_eq!(result.error_message.as_deref(), Some(explanation));
		});
	}

	#[test]
	fn preserves_sensitive_stop_reasons_with_a_descriptive_error_message() {
		run(|_| async {
			let model = model("anthropic", "claude-haiku-4-5");
			let response = sse_response(&stop_events("msg_sensitive", 12, json!({ "stop_reason": "sensitive" })));

			let result = stream(
				&model,
				&user_context("blocked request"),
				Some(client_options(FakeClient::new(response))),
			)
			.result()
			.await
			.unwrap();

			assert_eq!(result.stop_reason, StopReason::Error);
			assert_eq!(result.raw_stop_reason.as_deref(), Some("sensitive"));
			assert_eq!(
				result.error_message.as_deref(),
				Some("Provider stopped with: sensitive")
			);
		});
	}

	#[test]
	fn treats_message_delta_without_usage_as_a_no_op_for_usage_accumulation() {
		run(|_| async {
			let model = model("anthropic", "claude-haiku-4-5");
			let events: Vec<SseEvent> = minimal_events()
				.into_iter()
				.map(|(name, data)| {
					if name == "message_delta" {
						event(
							"message_delta",
							json!({ "type": "message_delta", "delta": { "stop_reason": "end_turn" } }),
						)
					} else {
						(name, data)
					}
				})
				.collect();

			let result = stream(
				&model,
				&user_context("Say hello."),
				Some(client_options(FakeClient::new(sse_response(&events)))),
			)
			.result()
			.await
			.unwrap();

			assert_eq!(result.stop_reason, StopReason::Stop);
			assert_eq!(result.error_message, None);
			assert_eq!(
				content_json(&result.content),
				json!([{ "type": "text", "text": "Hello" }])
			);
			assert_eq!(result.usage.input, 12.0);
			assert_eq!(result.usage.total_tokens, 12.0);
		});
	}

	#[test]
	fn ignores_unknown_sse_events_after_message_stop() {
		run(|_| async {
			let model = model("anthropic", "claude-haiku-4-5");
			let mut events = minimal_events();
			events.push(raw_event("done", "[DONE]"));
			events.push(raw_event("proxy.stats", "not json"));

			let result = stream(
				&model,
				&user_context("Say hello."),
				Some(client_options(FakeClient::new(sse_response(&events)))),
			)
			.result()
			.await
			.unwrap();

			assert_eq!(result.stop_reason, StopReason::Stop);
			assert_eq!(result.error_message, None);
			assert_eq!(
				content_json(&result.content),
				json!([{ "type": "text", "text": "Hello" }])
			);
		});
	}
}
