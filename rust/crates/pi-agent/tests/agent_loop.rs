//! Port of `packages/agent/test/agent-loop.test.ts`.
//!
//! Rust differences:
//! - Scripted stream functions create the stream, then push their final event from a spawned task
//!   (`queueMicrotask`). `setTimeout` and `sleep` run on the virtual clock of `pi_host::testing`, so durations are
//!   exact. Hand-resolved promises are `oneshot` channels.
//! - Identity checks (`toBe` on messages and models, `toContain` and `includes` on message arrays) compare by value:
//!   the loop works on copies of the caller's messages.
//! - `toMatchObject` is a subset match on the JSON form of the outcome.
//! - `Reflect.apply` without `streamFn` is a `None` stream function.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use futures::channel::oneshot;
use pi_agent::{
	AfterToolCallContext, AfterToolCallResult, AgentContext, AgentEvent, AgentEventSink, AgentLoopConfig,
	AgentLoopTurnUpdate, AgentMessage, AgentRequestUpdate, AgentTool, AgentToolCall, AgentToolCallOutcome,
	AgentToolResult, AgentToolUpdateCallback, AgentTurnContext, AgentTurnDecision, BeforeToolCallContext,
	BeforeToolCallResult, ConvertToLlmFn, PrepareRequestContext, RunToolCallOptions, StreamFn, ThinkingLevel,
	ToolCallHooks, ToolExecutionMode, agent_loop, agent_loop_continue, run_agent_loop, run_tool_call,
	set_default_stream_fn,
};
use pi_ai::types::{
	AssistantContent, AssistantMessage, AssistantMessageEvent, Message, Model, SimpleStreamOptions, StopReason,
	SystemMessage, SystemMessageContent, ThinkingLevel as ReasoningLevel, Tool, ToolResultMessage, TranscriptContext,
	Usage, UserContent, UserMessage, UserMessageContent,
};
use pi_ai::utils::event_stream::AssistantMessageEventStream;
use pi_host::{AbortSignal, LocalBoxFuture, PiError, PiResult};
use pi_schema::{arr, obj, str};
use serde_json::{Map, Value, json};

fn create_usage() -> Usage {
	serde_json::from_value(json!({
		"input": 0,
		"output": 0,
		"cacheRead": 0,
		"cacheWrite": 0,
		"totalTokens": 0,
		"cost": { "input": 0, "output": 0, "cacheRead": 0, "cacheWrite": 0, "total": 0 },
	}))
	.expect("usage literal")
}

fn create_model() -> Model {
	serde_json::from_value(json!({
		"id": "mock",
		"name": "mock",
		"api": "openai-responses",
		"provider": "openai",
		"baseUrl": "https://example.invalid",
		"reasoning": false,
		"input": ["text"],
		"cost": { "input": 0, "output": 0, "cacheRead": 0, "cacheWrite": 0 },
		"contextWindow": 8192,
		"maxTokens": 2048,
	}))
	.expect("model literal")
}

fn create_assistant_message(content: Vec<AssistantContent>, stop_reason: StopReason) -> AssistantMessage {
	AssistantMessage {
		content,
		api: "openai-responses".into(),
		provider: "openai".into(),
		model: "mock".into(),
		usage: create_usage(),
		stop_reason,
		timestamp: pi_host::now_ms(),
		..AssistantMessage::default()
	}
}

fn create_user_message(text: &str) -> UserMessage {
	UserMessage {
		content: UserMessageContent::Text(text.into()),
		timestamp: pi_host::now_ms(),
		..UserMessage::default()
	}
}

// Simple identity converter for tests - just passes through standard messages
fn identity_converter() -> ConvertToLlmFn {
	Rc::new(|messages: Vec<AgentMessage>| {
		let messages: Vec<Message> = messages
			.into_iter()
			.filter_map(|message| message.as_llm().cloned())
			.collect();
		Box::pin(async move { Ok(messages) })
	})
}

/// Assistant content blocks from their TS literal.
fn content(blocks: Value) -> Vec<AssistantContent> {
	serde_json::from_value(blocks).expect("assistant content literal")
}

/// A tool result from its TS literal.
fn tool_result(value: Value) -> AgentToolResult {
	serde_json::from_value(value).expect("tool result literal")
}

/// A tool call from its TS literal.
fn tool_call_of(id: &str, name: &str, args: Value) -> AgentToolCall {
	serde_json::from_value(json!({ "type": "toolCall", "id": id, "name": name, "arguments": args }))
		.expect("tool call literal")
}

fn as_stream_fn(
	stream_fn: impl Fn(
		&Model,
		&TranscriptContext,
		Option<SimpleStreamOptions>,
	) -> LocalBoxFuture<'static, PiResult<AssistantMessageEventStream>>
	+ 'static,
) -> StreamFn {
	Rc::new(stream_fn)
}

/// `const stream = createAssistantMessageEventStream(); queueMicrotask(() => stream.push(event())); return stream;`
fn respond_with(
	event: impl FnOnce() -> AssistantMessageEvent + 'static,
) -> LocalBoxFuture<'static, PiResult<AssistantMessageEventStream>> {
	let stream = AssistantMessageEventStream::new();
	let pusher = stream.clone();
	pi_host::spawn_local(async move { pusher.push(event()) });
	Box::pin(async move { Ok(stream) })
}

fn done(reason: StopReason, message: AssistantMessage) -> AssistantMessageEvent {
	AssistantMessageEvent::Done {
		reason,
		message: Rc::new(RefCell::new(message)),
	}
}

/// A synchronous emit callback.
fn sink(listener: impl Fn(AgentEvent) + 'static) -> AgentEventSink {
	Rc::new(move |event| {
		listener(event);
		Box::pin(async { Ok(()) })
	})
}

fn agent_tool(
	name: &str,
	label: &str,
	description: &str,
	parameters: Value,
	execute: impl Fn(
		String,
		Value,
		Option<AbortSignal>,
		Option<AgentToolUpdateCallback>,
	) -> LocalBoxFuture<'static, PiResult<AgentToolResult>>
	+ 'static,
) -> AgentTool {
	AgentTool {
		tool: Tool {
			name: name.into(),
			description: description.into(),
			parameters,
			..Tool::default()
		},
		label: label.into(),
		prepare_arguments: None,
		output_schema: None,
		execute: Rc::new(execute),
		replay: None,
		execution_mode: None,
	}
}

/// `obj({ value: str() })`.
fn value_schema() -> Value {
	obj([("value", str(Value::Null).into())], Value::Null)
}

/// `obj({})`.
fn empty_schema() -> Value {
	obj(Vec::<(&str, pi_schema::Property)>::new(), Value::Null)
}

/// `event.type`.
fn event_type(event: &AgentEvent) -> &'static str {
	match event {
		AgentEvent::AgentStart => "agent_start",
		AgentEvent::AgentEnd { .. } => "agent_end",
		AgentEvent::TurnStart => "turn_start",
		AgentEvent::TurnEnd { .. } => "turn_end",
		AgentEvent::MessageStart { .. } => "message_start",
		AgentEvent::MessageUpdate { .. } => "message_update",
		AgentEvent::MessageEnd { .. } => "message_end",
		AgentEvent::ToolExecutionStart { .. } => "tool_execution_start",
		AgentEvent::ToolExecutionUpdate { .. } => "tool_execution_update",
		AgentEvent::ToolExecutionEnd { .. } => "tool_execution_end",
	}
}

/// `message.content` of a user message whose content is a string.
fn user_text(message: &Message) -> Option<&str> {
	match message {
		Message::User(UserMessage {
			content: UserMessageContent::Text(text),
			..
		}) => Some(text),
		_ => None,
	}
}

fn tool_result_message(message: &AgentMessage) -> Option<&ToolResultMessage> {
	match message {
		AgentMessage::Llm(Message::ToolResult(tool_result)) => Some(tool_result),
		_ => None,
	}
}

fn roles(messages: &[AgentMessage]) -> Vec<&str> {
	messages.iter().map(AgentMessage::role).collect()
}

async fn drain(stream: &pi_ai::utils::event_stream::EventStream<AgentEvent, Vec<AgentMessage>>) -> Vec<AgentEvent> {
	let mut events = Vec::new();
	while let Some(event) = stream.next().await {
		events.push(event);
	}
	events
}

/// `expect(actual).toMatchObject(expected)`.
fn assert_match_object(actual: &Value, expected: &Value) {
	assert!(
		matches_object(actual, expected),
		"expected {actual} to match {expected}"
	);
}

fn matches_object(actual: &Value, expected: &Value) -> bool {
	match (actual, expected) {
		(Value::Object(actual), Value::Object(expected)) => expected
			.iter()
			.all(|(key, value)| actual.get(key).is_some_and(|actual| matches_object(actual, value))),
		(Value::Array(actual), Value::Array(expected)) => {
			actual.len() == expected.len()
				&& actual
					.iter()
					.zip(expected)
					.all(|(actual, expected)| matches_object(actual, expected))
		}
		_ => actual == expected,
	}
}

mod default_stream_function_compatibility {
	use super::*;

	/// The `finally` block.
	struct ResetDefaultStreamFn;

	impl Drop for ResetDefaultStreamFn {
		fn drop(&mut self) {
			set_default_stream_fn(None);
		}
	}

	#[test]
	fn uses_the_configured_default_when_a_legacy_caller_omits_stream_fn() {
		pi_host::testing::run(|_host| async move {
			let calls = Rc::new(Cell::new(0));
			let counter = calls.clone();
			set_default_stream_fn(Some(as_stream_fn(move |_, _, _| {
				counter.set(counter.get() + 1);
				respond_with(|| {
					done(
						StopReason::Stop,
						create_assistant_message(
							content(json!([{ "type": "text", "text": "fallback" }])),
							StopReason::Stop,
						),
					)
				})
			})));

			let _reset = ResetDefaultStreamFn;
			let context = AgentContext {
				messages: vec![],
				tools: Some(vec![]),
			};
			let config = AgentLoopConfig::new(create_model(), identity_converter());
			let stream = agent_loop(vec![create_user_message("Hello").into()], context, config, None, None);

			stream.result().await;
			assert_eq!(calls.get(), 1);
		});
	}
}

mod agent_loop_with_agent_message {
	use super::*;

	#[test]
	fn should_emit_events_with_agent_message_types() {
		pi_host::testing::run(|_host| async move {
			let context = AgentContext {
				messages: vec![],
				tools: Some(vec![]),
			};

			let user_prompt: AgentMessage = create_user_message("Hello").into();

			let config = AgentLoopConfig::new(create_model(), identity_converter());

			let stream_fn = as_stream_fn(|_, _, _| {
				respond_with(|| {
					let message = create_assistant_message(
						content(json!([{ "type": "text", "text": "Hi there!" }])),
						StopReason::Stop,
					);
					done(StopReason::Stop, message)
				})
			});

			let stream = agent_loop(vec![user_prompt], context, config, None, Some(stream_fn));

			let events = drain(&stream).await;

			let messages = stream.result().await.expect("agent loop result");

			// Should have user message and assistant message
			assert_eq!(messages.len(), 2);
			assert_eq!(messages[0].role(), "user");
			assert_eq!(messages[1].role(), "assistant");

			// Verify event sequence
			let event_types: Vec<&str> = events.iter().map(event_type).collect();
			assert!(event_types.contains(&"agent_start"));
			assert!(event_types.contains(&"turn_start"));
			assert!(event_types.contains(&"message_start"));
			assert!(event_types.contains(&"message_end"));
			assert!(event_types.contains(&"turn_end"));
			assert!(event_types.contains(&"agent_end"));
		});
	}

	#[test]
	fn should_build_provider_context_exclusively_from_transcript_messages() {
		pi_host::testing::run(|_host| async move {
			let initial_system = SystemMessage {
				content: SystemMessageContent::Text("Transcript prompt".into()),
				tools_added: Some(vec![]),
				timestamp: 1.0,
				..SystemMessage::default()
			};
			let context = AgentContext {
				messages: vec![],
				tools: Some(vec![]),
			};
			let config = AgentLoopConfig::new(create_model(), identity_converter());
			let expected_system = initial_system.clone();
			let stream = agent_loop(
				vec![initial_system.into(), create_user_message("Hello").into()],
				context,
				config,
				None,
				Some(as_stream_fn(move |_model, provider_context, _| {
					// The provider receives a transcript: no top-level prompt or tool fields.
					let provider_context_json = serde_json::to_value(provider_context).expect("context serializes");
					let keys: Vec<&String> = provider_context_json
						.as_object()
						.expect("context is an object")
						.keys()
						.collect();
					assert_eq!(keys, vec!["messages"]);
					assert_eq!(provider_context.messages[0], Message::System(expected_system.clone()));
					respond_with(|| {
						done(
							StopReason::Stop,
							create_assistant_message(
								content(json!([{ "type": "text", "text": "done" }])),
								StopReason::Stop,
							),
						)
					})
				})),
			);

			stream.result().await;
		});
	}

	#[test]
	fn should_handle_custom_message_types_via_convert_to_llm() {
		pi_host::testing::run(|_host| async move {
			// Create a custom message type
			let notification = json!({
				"role": "notification",
				"text": "This is a notification",
				"timestamp": pi_host::now_ms(),
			});

			let context = AgentContext {
				messages: vec![AgentMessage::Custom(notification)], // Custom message in context
				tools: Some(vec![]),
			};

			let user_prompt: AgentMessage = create_user_message("Hello").into();

			let converted_messages: Rc<RefCell<Vec<Message>>> = Rc::new(RefCell::new(Vec::new()));
			let converted = converted_messages.clone();
			let config = AgentLoopConfig::new(
				create_model(),
				Rc::new(move |messages: Vec<AgentMessage>| {
					// Filter out notifications, convert rest
					let filtered: Vec<Message> = messages
						.into_iter()
						.filter(|message| message.role() != "notification")
						.filter_map(|message| match message {
							AgentMessage::Llm(
								message @ (Message::User(_) | Message::Assistant(_) | Message::ToolResult(_)),
							) => Some(message),
							_ => None,
						})
						.collect();
					*converted.borrow_mut() = filtered.clone();
					Box::pin(async move { Ok(filtered) })
				}),
			);

			let stream_fn = as_stream_fn(|_, _, _| {
				respond_with(|| {
					let message = create_assistant_message(
						content(json!([{ "type": "text", "text": "Response" }])),
						StopReason::Stop,
					);
					done(StopReason::Stop, message)
				})
			});

			let stream = agent_loop(vec![user_prompt], context, config, None, Some(stream_fn));

			let _events = drain(&stream).await;

			// The notification should have been filtered out in convertToLlm
			assert_eq!(converted_messages.borrow().len(), 1); // Only user message
			assert!(matches!(converted_messages.borrow()[0], Message::User(_)));
		});
	}

	#[test]
	fn should_apply_transform_context_before_convert_to_llm() {
		pi_host::testing::run(|_host| async move {
			let context = AgentContext {
				messages: vec![
					create_user_message("old message 1").into(),
					create_assistant_message(
						content(json!([{ "type": "text", "text": "old response 1" }])),
						StopReason::Stop,
					)
					.into(),
					create_user_message("old message 2").into(),
					create_assistant_message(
						content(json!([{ "type": "text", "text": "old response 2" }])),
						StopReason::Stop,
					)
					.into(),
				],
				tools: Some(vec![]),
			};

			let user_prompt: AgentMessage = create_user_message("new message").into();

			let transformed_messages: Rc<RefCell<Vec<AgentMessage>>> = Rc::new(RefCell::new(Vec::new()));
			let converted_messages: Rc<RefCell<Vec<Message>>> = Rc::new(RefCell::new(Vec::new()));

			let transformed = transformed_messages.clone();
			let converted = converted_messages.clone();
			let config = AgentLoopConfig {
				transform_context: Some(Rc::new(move |messages: Vec<AgentMessage>, _signal| {
					// Keep only last 2 messages (prune old ones)
					let kept = messages[messages.len().saturating_sub(2)..].to_vec();
					*transformed.borrow_mut() = kept.clone();
					Box::pin(async move { Ok(kept) })
				})),
				..AgentLoopConfig::new(
					create_model(),
					Rc::new(move |messages: Vec<AgentMessage>| {
						let filtered: Vec<Message> = messages
							.into_iter()
							.filter_map(|message| match message {
								AgentMessage::Llm(
									message @ (Message::User(_) | Message::Assistant(_) | Message::ToolResult(_)),
								) => Some(message),
								_ => None,
							})
							.collect();
						*converted.borrow_mut() = filtered.clone();
						Box::pin(async move { Ok(filtered) })
					}),
				)
			};

			let stream_fn = as_stream_fn(|_, _, _| {
				respond_with(|| {
					let message = create_assistant_message(
						content(json!([{ "type": "text", "text": "Response" }])),
						StopReason::Stop,
					);
					done(StopReason::Stop, message)
				})
			});

			let stream = agent_loop(vec![user_prompt], context, config, None, Some(stream_fn));

			drain(&stream).await;

			// transformContext should have been called first, keeping only last 2
			assert_eq!(transformed_messages.borrow().len(), 2);
			// Then convertToLlm receives the pruned messages
			assert_eq!(converted_messages.borrow().len(), 2);
		});
	}

	#[test]
	fn should_handle_tool_calls_and_results() {
		pi_host::testing::run(|_host| async move {
			let tool_schema = value_schema();
			let executed: Rc<RefCell<Vec<String>>> = Rc::new(RefCell::new(Vec::new()));
			let tool_usage: Usage = serde_json::from_value(json!({
				"input": 1,
				"output": 2,
				"cacheRead": 3,
				"cacheWrite": 4,
				"totalTokens": 10,
				"cost": { "input": 0.1, "output": 0.2, "cacheRead": 0.3, "cacheWrite": 0.4, "total": 1 },
			}))
			.expect("usage literal");
			let patched_tool_usage: Usage = serde_json::from_value(json!({
				"input": 5,
				"output": 6,
				"cacheRead": 7,
				"cacheWrite": 8,
				"totalTokens": 26,
				"cost": { "input": 0.5, "output": 0.6, "cacheRead": 0.7, "cacheWrite": 0.8, "total": 2.6 },
			}))
			.expect("usage literal");
			let observed_tool_usage: Rc<RefCell<Option<Usage>>> = Rc::new(RefCell::new(None));
			let tool = {
				let executed = executed.clone();
				let tool_usage = tool_usage.clone();
				agent_tool(
					"echo",
					"Echo",
					"Echo tool",
					tool_schema,
					move |_tool_call_id, params, _, _| {
						let executed = executed.clone();
						let tool_usage = tool_usage.clone();
						Box::pin(async move {
							let value = params["value"].as_str().unwrap_or_default().to_string();
							executed.borrow_mut().push(value.clone());
							Ok(AgentToolResult {
								usage: Some(tool_usage),
								..tool_result(json!({
									"content": [{ "type": "text", "text": format!("echoed: {value}") }],
									"details": { "value": value },
								}))
							})
						})
					},
				)
			};

			let context = AgentContext {
				messages: vec![],
				tools: Some(vec![tool]),
			};

			let user_prompt: AgentMessage = create_user_message("echo something").into();

			let observed = observed_tool_usage.clone();
			let patched = patched_tool_usage.clone();
			let config = AgentLoopConfig {
				after_tool_call: Some(Rc::new(move |hook: AfterToolCallContext, _signal| {
					*observed.borrow_mut() = hook.result.usage.clone();
					let patched = patched.clone();
					Box::pin(async move {
						Ok(Some(AfterToolCallResult {
							usage: Some(patched),
							..AfterToolCallResult::default()
						}))
					})
				})),
				..AgentLoopConfig::new(create_model(), identity_converter())
			};

			let call_index = Rc::new(Cell::new(0));
			let stream_fn = as_stream_fn(move |_, _, _| {
				let call_index = call_index.clone();
				respond_with(move || {
					let event = if call_index.get() == 0 {
						// First call: return tool call
						let message = create_assistant_message(
							content(
								json!([{ "type": "toolCall", "id": "tool-1", "name": "echo", "arguments": { "value": "hello" } }]),
							),
							StopReason::ToolUse,
						);
						done(StopReason::ToolUse, message)
					} else {
						// Second call: return final response
						let message = create_assistant_message(
							content(json!([{ "type": "text", "text": "done" }])),
							StopReason::Stop,
						);
						done(StopReason::Stop, message)
					};
					call_index.set(call_index.get() + 1);
					event
				})
			});

			let stream = agent_loop(vec![user_prompt], context, config, None, Some(stream_fn));

			let events = drain(&stream).await;

			// Tool should have been executed
			assert_eq!(*executed.borrow(), vec!["hello"]);

			// Should have tool execution events
			let tool_start = events
				.iter()
				.find(|event| matches!(event, AgentEvent::ToolExecutionStart { .. }));
			let tool_end = events
				.iter()
				.find(|event| matches!(event, AgentEvent::ToolExecutionEnd { .. }));
			assert!(tool_start.is_some());
			assert!(tool_end.is_some());
			if let Some(AgentEvent::ToolExecutionEnd { is_error, .. }) = tool_end {
				assert!(!is_error);
			}
			assert_eq!(*observed_tool_usage.borrow(), Some(tool_usage));
			let messages = stream.result().await.expect("agent loop result");
			let tool_result = messages.iter().find_map(tool_result_message);
			assert_eq!(
				tool_result.and_then(|tool_result| tool_result.usage.clone()),
				Some(patched_tool_usage)
			);
		});
	}

	// #10549
	#[test]
	fn records_how_long_execute_took_on_the_tool_result_excluding_hooks() {
		pi_host::testing::run(|_host| async move {
			let tool_schema = value_schema();
			let tool = agent_tool(
				"echo",
				"Echo",
				"Echo tool",
				tool_schema,
				|_tool_call_id, params, _, _| {
					Box::pin(async move {
						pi_host::sleep(30.0, None).await?;
						Ok(tool_result(json!({
							"content": [{ "type": "text", "text": params["value"] }],
							"details": { "value": params["value"] },
						})))
					})
				},
			);
			let config = AgentLoopConfig {
				before_tool_call: Some(Rc::new(|hook: BeforeToolCallContext, _signal| {
					Box::pin(async move {
						pi_host::sleep(100.0, None).await?;
						Ok((hook.tool_call.id == "blocked").then(|| BeforeToolCallResult {
							block: Some(true),
							reason: Some("no".into()),
							..BeforeToolCallResult::default()
						}))
					})
				})),
				..AgentLoopConfig::new(create_model(), identity_converter())
			};
			let call_index = Rc::new(Cell::new(0));
			let stream_fn = as_stream_fn(move |_, _, _| {
				let call_index = call_index.clone();
				respond_with(move || {
					let message = if call_index.get() == 0 {
						create_assistant_message(
							content(json!([
								{ "type": "toolCall", "id": "ran", "name": "echo", "arguments": { "value": "a" } },
								{ "type": "toolCall", "id": "blocked", "name": "echo", "arguments": { "value": "b" } },
							])),
							StopReason::ToolUse,
						)
					} else {
						create_assistant_message(content(json!([{ "type": "text", "text": "done" }])), StopReason::Stop)
					};
					let event = done(
						if call_index.get() == 0 {
							StopReason::ToolUse
						} else {
							StopReason::Stop
						},
						message,
					);
					call_index.set(call_index.get() + 1);
					event
				})
			});
			let stream = agent_loop(
				vec![create_user_message("go").into()],
				AgentContext {
					messages: vec![],
					tools: Some(vec![tool]),
				},
				config,
				None,
				Some(stream_fn),
			);
			drain(&stream).await;
			let messages = stream.result().await.expect("agent loop result");
			let results: Vec<&ToolResultMessage> = messages.iter().filter_map(tool_result_message).collect();
			let (ran, blocked) = (results.first(), results.get(1));
			let ran_duration_ms = ran.and_then(|ran| ran.duration_ms).expect("ran has durationMs");
			assert!(ran_duration_ms >= 25.0);
			assert!(ran_duration_ms < 100.0);
			assert_eq!(blocked.map(|blocked| blocked.is_error), Some(true));
			let blocked_json = serde_json::to_value(blocked.expect("blocked result")).expect("tool result serializes");
			assert!(blocked_json.get("durationMs").is_none());
		});
	}

	#[test]
	fn should_not_execute_tool_calls_from_a_length_truncated_assistant_message() {
		pi_host::testing::run(|_host| async move {
			let tool_schema = value_schema();
			let executed: Rc<RefCell<Vec<String>>> = Rc::new(RefCell::new(Vec::new()));
			let tool = {
				let executed = executed.clone();
				agent_tool(
					"echo",
					"Echo",
					"Echo tool",
					tool_schema,
					move |_tool_call_id, params, _, _| {
						let executed = executed.clone();
						Box::pin(async move {
							let value = params["value"].as_str().unwrap_or_default().to_string();
							executed.borrow_mut().push(value.clone());
							Ok(tool_result(json!({
								"content": [{ "type": "text", "text": format!("echoed: {value}") }],
								"details": { "value": value },
							})))
						})
					},
				)
			};

			let context = AgentContext {
				messages: vec![],
				tools: Some(vec![tool]),
			};

			let config = AgentLoopConfig::new(create_model(), identity_converter());

			let call_index = Rc::new(Cell::new(0));
			let counter = call_index.clone();
			let stream_fn = as_stream_fn(move |_, _, _| {
				let call_index = counter.clone();
				respond_with(move || {
					let event = if call_index.get() == 0 {
						// Output hit the token limit mid tool call. The salvage parser can
						// produce arguments that validate but are silently truncated, so
						// nothing in this message may execute.
						let message = create_assistant_message(
							content(
								json!([{ "type": "toolCall", "id": "tool-1", "name": "echo", "arguments": { "value": "hel" } }]),
							),
							StopReason::Length,
						);
						done(StopReason::Length, message)
					} else {
						let message = create_assistant_message(
							content(json!([{ "type": "text", "text": "done" }])),
							StopReason::Stop,
						);
						done(StopReason::Stop, message)
					};
					call_index.set(call_index.get() + 1);
					event
				})
			});

			let stream = agent_loop(
				vec![create_user_message("echo something").into()],
				context,
				config,
				None,
				Some(stream_fn),
			);
			let events = drain(&stream).await;

			// The tool must never execute with potentially truncated arguments.
			assert!(executed.borrow().is_empty());

			let tool_end = events
				.iter()
				.find(|event| matches!(event, AgentEvent::ToolExecutionEnd { .. }));
			assert!(tool_end.is_some());
			if let Some(AgentEvent::ToolExecutionEnd { is_error, result, .. }) = tool_end {
				assert!(is_error);
				let text = result.content.iter().find_map(|content| match content {
					UserContent::Text(text) => Some(text.text.as_str()),
					_ => None,
				});
				assert!(text.unwrap_or("").contains("output token limit"));
			}

			// The loop continues so the model can re-issue the tool call.
			assert_eq!(call_index.get(), 2);
			let messages = stream.result().await.expect("agent loop result");
			assert_eq!(messages[messages.len() - 1].role(), "assistant");
		});
	}

	#[test]
	fn should_execute_mutated_before_tool_call_args_without_revalidation() {
		pi_host::testing::run(|_host| async move {
			let tool_schema = value_schema();
			let executed: Rc<RefCell<Vec<Value>>> = Rc::new(RefCell::new(Vec::new()));
			let tool = {
				let executed = executed.clone();
				agent_tool(
					"echo",
					"Echo",
					"Echo tool",
					tool_schema,
					move |_tool_call_id, params, _, _| {
						let executed = executed.clone();
						Box::pin(async move {
							executed.borrow_mut().push(params["value"].clone());
							let shown = match &params["value"] {
								Value::String(value) => value.clone(),
								value => value.to_string(),
							};
							Ok(tool_result(json!({
								"content": [{ "type": "text", "text": format!("echoed: {shown}") }],
								"details": { "value": params["value"] },
							})))
						})
					},
				)
			};

			let context = AgentContext {
				messages: vec![],
				tools: Some(vec![tool]),
			};

			let user_prompt: AgentMessage = create_user_message("echo something").into();

			let config = AgentLoopConfig {
				before_tool_call: Some(Rc::new(|hook: BeforeToolCallContext, _signal| {
					Box::pin(async move {
						hook.args.borrow_mut()["value"] = json!(123);
						Ok(None)
					})
				})),
				..AgentLoopConfig::new(create_model(), identity_converter())
			};

			let call_index = Rc::new(Cell::new(0));
			let stream_fn = as_stream_fn(move |_, _, _| {
				let call_index = call_index.clone();
				respond_with(move || {
					let event = if call_index.get() == 0 {
						let message = create_assistant_message(
							content(
								json!([{ "type": "toolCall", "id": "tool-1", "name": "echo", "arguments": { "value": "hello" } }]),
							),
							StopReason::ToolUse,
						);
						done(StopReason::ToolUse, message)
					} else {
						let message = create_assistant_message(
							content(json!([{ "type": "text", "text": "done" }])),
							StopReason::Stop,
						);
						done(StopReason::Stop, message)
					};
					call_index.set(call_index.get() + 1);
					event
				})
			});

			let stream = agent_loop(vec![user_prompt], context, config, None, Some(stream_fn));
			drain(&stream).await;

			assert_eq!(*executed.borrow(), vec![json!(123)]);
		});
	}

	#[test]
	fn should_prepare_tool_arguments_for_validation() {
		pi_host::testing::run(|_host| async move {
			let replace_schema = obj(
				[
					("oldText", str(Value::Null).into()),
					("newText", str(Value::Null).into()),
				],
				Value::Null,
			);
			let tool_schema = obj([("edits", arr(replace_schema, Value::Null).into())], Value::Null);
			let executed: Rc<RefCell<Vec<Value>>> = Rc::new(RefCell::new(Vec::new()));
			let mut tool = {
				let executed = executed.clone();
				agent_tool(
					"edit",
					"Edit",
					"Edit tool",
					tool_schema,
					move |_tool_call_id, params, _, _| {
						let executed = executed.clone();
						Box::pin(async move {
							executed.borrow_mut().push(params["edits"].clone());
							let count = params["edits"].as_array().map_or(0, Vec::len);
							Ok(tool_result(json!({
								"content": [{ "type": "text", "text": format!("edited {count}") }],
								"details": { "count": count },
							})))
						})
					},
				)
			};
			tool.prepare_arguments = Some(Rc::new(|args: Value| {
				// The loop always passes the tool-call arguments object, so the TS `typeof args !== "object"` branch
				// cannot occur here.
				let Value::Object(input) = args else {
					return Err(PiError::new("tool-call arguments are an object"));
				};
				let (Some(Value::String(old_text)), Some(Value::String(new_text))) =
					(input.get("oldText"), input.get("newText"))
				else {
					return Ok(input);
				};
				let mut edits = input
					.get("edits")
					.and_then(Value::as_array)
					.cloned()
					.unwrap_or_default();
				edits.push(json!({ "oldText": old_text, "newText": new_text }));
				Ok(Map::from_iter([("edits".to_string(), Value::Array(edits))]))
			}));

			let context = AgentContext {
				messages: vec![],
				tools: Some(vec![tool]),
			};

			let user_prompt: AgentMessage = create_user_message("edit something").into();
			let config = AgentLoopConfig::new(create_model(), identity_converter());

			let call_index = Rc::new(Cell::new(0));
			let stream_fn = as_stream_fn(move |_, _, _| {
				let call_index = call_index.clone();
				respond_with(move || {
					let event = if call_index.get() == 0 {
						let message = create_assistant_message(
							content(json!([
								{
									"type": "toolCall",
									"id": "tool-1",
									"name": "edit",
									"arguments": { "oldText": "before", "newText": "after" },
								},
							])),
							StopReason::ToolUse,
						);
						done(StopReason::ToolUse, message)
					} else {
						let message = create_assistant_message(
							content(json!([{ "type": "text", "text": "done" }])),
							StopReason::Stop,
						);
						done(StopReason::Stop, message)
					};
					call_index.set(call_index.get() + 1);
					event
				})
			});

			let stream = agent_loop(vec![user_prompt], context, config, None, Some(stream_fn));
			drain(&stream).await;

			assert_eq!(
				*executed.borrow(),
				vec![json!([{ "oldText": "before", "newText": "after" }])]
			);
		});
	}

	/// The scripted provider of the parallel and sequential tests: two tool calls, then a final response. `release`
	/// resolves 20 ms after the first response.
	fn two_tool_calls_then_done(
		name: &'static str,
		values: [&'static str; 2],
		release: Rc<RefCell<Option<oneshot::Sender<()>>>>,
	) -> StreamFn {
		let call_index = Rc::new(Cell::new(0));
		as_stream_fn(move |_, _, _| {
			let call_index = call_index.clone();
			let release = release.clone();
			respond_with(move || {
				let event = if call_index.get() == 0 {
					let message = create_assistant_message(
						content(json!([
							{ "type": "toolCall", "id": "tool-1", "name": name, "arguments": { "value": values[0] } },
							{ "type": "toolCall", "id": "tool-2", "name": name, "arguments": { "value": values[1] } },
						])),
						StopReason::ToolUse,
					);
					pi_host::spawn_local(async move {
						let _ = pi_host::sleep(20.0, None).await;
						let release = release.borrow_mut().take();
						if let Some(release) = release {
							let _ = release.send(());
						}
					});
					done(StopReason::ToolUse, message)
				} else {
					let message = create_assistant_message(
						content(json!([{ "type": "text", "text": "done" }])),
						StopReason::Stop,
					);
					done(StopReason::Stop, message)
				};
				call_index.set(call_index.get() + 1);
				event
			})
		})
	}

	/// The `echo`/`slow` tool of the parallel and sequential tests: `first` waits for `first_done`, `second` records
	/// whether it ran before `first` finished.
	fn waiting_tool(
		name: &str,
		label: &str,
		description: &str,
		prefix: &'static str,
		first_done: oneshot::Receiver<()>,
		first_resolved: Rc<Cell<bool>>,
		parallel_observed: Rc<Cell<bool>>,
	) -> AgentTool {
		let first_done = Rc::new(RefCell::new(Some(first_done)));
		agent_tool(
			name,
			label,
			description,
			value_schema(),
			move |_tool_call_id, params, _, _| {
				let first_done = first_done.clone();
				let first_resolved = first_resolved.clone();
				let parallel_observed = parallel_observed.clone();
				Box::pin(async move {
					let value = params["value"].as_str().unwrap_or_default().to_string();
					if value == "first" {
						let receiver = first_done.borrow_mut().take();
						if let Some(receiver) = receiver {
							let _ = receiver.await;
						}
						first_resolved.set(true);
					}
					if value == "second" && !first_resolved.get() {
						parallel_observed.set(true);
					}
					Ok(tool_result(json!({
						"content": [{ "type": "text", "text": format!("{prefix}{value}") }],
						"details": { "value": value },
					})))
				})
			},
		)
	}

	#[test]
	fn should_emit_tool_execution_end_in_completion_order_but_persist_tool_results_in_source_order() {
		pi_host::testing::run(|_host| async move {
			let first_resolved = Rc::new(Cell::new(false));
			let parallel_observed = Rc::new(Cell::new(false));
			let (release_first, first_done) = oneshot::channel::<()>();
			let release_first = Rc::new(RefCell::new(Some(release_first)));

			let tool = waiting_tool(
				"echo",
				"Echo",
				"Echo tool",
				"echoed: ",
				first_done,
				first_resolved.clone(),
				parallel_observed.clone(),
			);

			let context = AgentContext {
				messages: vec![],
				tools: Some(vec![tool]),
			};

			let user_prompt: AgentMessage = create_user_message("echo both").into();
			let config = AgentLoopConfig {
				tool_execution: Some(ToolExecutionMode::Parallel),
				..AgentLoopConfig::new(create_model(), identity_converter())
			};

			let stream = agent_loop(
				vec![user_prompt],
				context,
				config,
				None,
				Some(two_tool_calls_then_done("echo", ["first", "second"], release_first)),
			);

			let events = drain(&stream).await;

			let tool_execution_end_ids: Vec<&str> = events
				.iter()
				.filter_map(|event| match event {
					AgentEvent::ToolExecutionEnd { tool_call_id, .. } => Some(tool_call_id.as_str()),
					_ => None,
				})
				.collect();
			let tool_result_ids: Vec<&str> = events
				.iter()
				.filter_map(|event| match event {
					AgentEvent::MessageEnd { message } => tool_result_message(message),
					_ => None,
				})
				.map(|tool_result| tool_result.tool_call_id.as_str())
				.collect();
			let turn_tool_result_ids: Vec<&str> = events
				.iter()
				.flat_map(|event| match event {
					AgentEvent::TurnEnd { tool_results, .. } => tool_results.as_slice(),
					_ => &[],
				})
				.map(|tool_result| tool_result.tool_call_id.as_str())
				.collect();

			assert!(parallel_observed.get());
			assert_eq!(tool_execution_end_ids, vec!["tool-2", "tool-1"]);
			assert_eq!(tool_result_ids, vec!["tool-1", "tool-2"]);
			assert_eq!(turn_tool_result_ids, vec!["tool-1", "tool-2"]);
		});
	}

	#[test]
	fn should_inject_queued_messages_after_all_tool_calls_complete() {
		pi_host::testing::run(|_host| async move {
			let tool_schema = value_schema();
			let executed: Rc<RefCell<Vec<String>>> = Rc::new(RefCell::new(Vec::new()));
			let tool = {
				let executed = executed.clone();
				agent_tool(
					"echo",
					"Echo",
					"Echo tool",
					tool_schema,
					move |_tool_call_id, params, _, _| {
						let executed = executed.clone();
						Box::pin(async move {
							let value = params["value"].as_str().unwrap_or_default().to_string();
							executed.borrow_mut().push(value.clone());
							Ok(tool_result(json!({
								"content": [{ "type": "text", "text": format!("ok:{value}") }],
								"details": { "value": value },
							})))
						})
					},
				)
			};

			let context = AgentContext {
				messages: vec![],
				tools: Some(vec![tool]),
			};

			let user_prompt: AgentMessage = create_user_message("start").into();
			let queued_user_message: AgentMessage = create_user_message("interrupt").into();

			let queued_delivered = Rc::new(Cell::new(false));
			let call_index = Rc::new(Cell::new(0));
			let saw_interrupt_in_context = Rc::new(Cell::new(false));

			let polled_executed = executed.clone();
			let config = AgentLoopConfig {
				tool_execution: Some(ToolExecutionMode::Sequential),
				get_steering_messages: Some(Rc::new(move || {
					let executed = polled_executed.clone();
					let queued_delivered = queued_delivered.clone();
					let queued_user_message = queued_user_message.clone();
					Box::pin(async move {
						// Return steering message after tool execution has started.
						if !executed.borrow().is_empty() && !queued_delivered.get() {
							queued_delivered.set(true);
							return Ok(vec![queued_user_message]);
						}
						Ok(vec![])
					})
				})),
				..AgentLoopConfig::new(create_model(), identity_converter())
			};

			let saw_interrupt = saw_interrupt_in_context.clone();
			let stream = agent_loop(
				vec![user_prompt],
				context,
				config,
				None,
				Some(as_stream_fn(move |_model, ctx, _options| {
					// Check if interrupt message is in context on second call
					if call_index.get() == 1 {
						saw_interrupt.set(
							ctx.messages
								.iter()
								.any(|message| user_text(message) == Some("interrupt")),
						);
					}

					let call_index = call_index.clone();
					respond_with(move || {
						let event = if call_index.get() == 0 {
							// First call: return two tool calls
							let message = create_assistant_message(
								content(json!([
									{ "type": "toolCall", "id": "tool-1", "name": "echo", "arguments": { "value": "first" } },
									{ "type": "toolCall", "id": "tool-2", "name": "echo", "arguments": { "value": "second" } },
								])),
								StopReason::ToolUse,
							);
							done(StopReason::ToolUse, message)
						} else {
							// Second call: return final response
							let message = create_assistant_message(
								content(json!([{ "type": "text", "text": "done" }])),
								StopReason::Stop,
							);
							done(StopReason::Stop, message)
						};
						call_index.set(call_index.get() + 1);
						event
					})
				})),
			);

			let events = drain(&stream).await;

			// Both tools should execute before steering is injected
			assert_eq!(*executed.borrow(), vec!["first", "second"]);

			let tool_ends: Vec<bool> = events
				.iter()
				.filter_map(|event| match event {
					AgentEvent::ToolExecutionEnd { is_error, .. } => Some(*is_error),
					_ => None,
				})
				.collect();
			assert_eq!(tool_ends.len(), 2);
			assert!(!tool_ends[0]);
			assert!(!tool_ends[1]);

			// Queued message should appear in events after both tool result messages
			let event_sequence: Vec<String> = events
				.iter()
				.filter_map(|event| {
					let AgentEvent::MessageStart { message } = event else {
						return None;
					};
					if let Some(tool_result) = tool_result_message(message) {
						return Some(format!("tool:{}", tool_result.tool_call_id));
					}
					message.as_llm().and_then(user_text).map(str::to_string)
				})
				.collect();
			let index_of = |entry: &str| event_sequence.iter().position(|item| item == entry);
			assert!(event_sequence.contains(&"interrupt".to_string()));
			assert!(index_of("tool:tool-1") < index_of("interrupt"));
			assert!(index_of("tool:tool-2") < index_of("interrupt"));

			// Interrupt message should be in context when second LLM call is made
			assert!(saw_interrupt_in_context.get());
		});
	}

	#[test]
	fn should_force_sequential_execution_when_a_tool_has_execution_mode_sequential_even_with_default_parallel_config() {
		pi_host::testing::run(|_host| async move {
			let first_resolved = Rc::new(Cell::new(false));
			let parallel_observed = Rc::new(Cell::new(false));
			let (release_first, first_done) = oneshot::channel::<()>();
			let release_first = Rc::new(RefCell::new(Some(release_first)));

			let mut slow_tool = waiting_tool(
				"slow",
				"Slow",
				"Slow tool",
				"slow: ",
				first_done,
				first_resolved.clone(),
				parallel_observed.clone(),
			);
			slow_tool.execution_mode = Some(ToolExecutionMode::Sequential);

			let context = AgentContext {
				messages: vec![],
				tools: Some(vec![slow_tool]),
			};

			let user_prompt: AgentMessage = create_user_message("run both").into();
			// config is parallel (default), but tool forces sequential
			let config = AgentLoopConfig::new(create_model(), identity_converter());

			let stream = agent_loop(
				vec![user_prompt],
				context,
				config,
				None,
				Some(two_tool_calls_then_done("slow", ["first", "second"], release_first)),
			);

			let events = drain(&stream).await;

			// With sequential execution, second tool should NOT start before first finishes
			assert!(!parallel_observed.get());

			let tool_result_ids: Vec<&str> = events
				.iter()
				.filter_map(|event| match event {
					AgentEvent::MessageEnd { message } => tool_result_message(message),
					_ => None,
				})
				.map(|tool_result| tool_result.tool_call_id.as_str())
				.collect();
			assert_eq!(tool_result_ids, vec!["tool-1", "tool-2"]);
		});
	}

	#[test]
	fn should_force_sequential_execution_when_one_of_multiple_tools_has_execution_mode_sequential() {
		pi_host::testing::run(|_host| async move {
			let execution_order: Rc<RefCell<Vec<String>>> = Rc::new(RefCell::new(Vec::new()));
			let (release_slow, slow_done) = oneshot::channel::<()>();
			let release_slow = Rc::new(RefCell::new(Some(release_slow)));
			let slow_done = Rc::new(RefCell::new(Some(slow_done)));

			let mut slow_tool = {
				let execution_order = execution_order.clone();
				agent_tool(
					"slow",
					"Slow",
					"Slow tool",
					value_schema(),
					move |_tool_call_id, params, _, _| {
						let execution_order = execution_order.clone();
						let slow_done = slow_done.clone();
						Box::pin(async move {
							let value = params["value"].as_str().unwrap_or_default().to_string();
							execution_order.borrow_mut().push(format!("slow:{value}"));
							if value == "a" {
								let receiver = slow_done.borrow_mut().take();
								if let Some(receiver) = receiver {
									let _ = receiver.await;
								}
							}
							Ok(tool_result(json!({
								"content": [{ "type": "text", "text": format!("slow: {value}") }],
								"details": { "value": value },
							})))
						})
					},
				)
			};
			slow_tool.execution_mode = Some(ToolExecutionMode::Sequential);

			// no executionMode = defaults to parallel
			let fast_tool = {
				let execution_order = execution_order.clone();
				agent_tool(
					"fast",
					"Fast",
					"Fast tool",
					value_schema(),
					move |_tool_call_id, params, _, _| {
						let execution_order = execution_order.clone();
						Box::pin(async move {
							let value = params["value"].as_str().unwrap_or_default().to_string();
							execution_order.borrow_mut().push(format!("fast:{value}"));
							Ok(tool_result(json!({
								"content": [{ "type": "text", "text": format!("fast: {value}") }],
								"details": { "value": value },
							})))
						})
					},
				)
			};

			let context = AgentContext {
				messages: vec![],
				tools: Some(vec![slow_tool, fast_tool]),
			};

			let user_prompt: AgentMessage = create_user_message("run both").into();
			// parallel by default, but slowTool forces sequential
			let config = AgentLoopConfig::new(create_model(), identity_converter());

			let call_index = Rc::new(Cell::new(0));
			let stream = agent_loop(
				vec![user_prompt],
				context,
				config,
				None,
				Some(as_stream_fn(move |_, _, _| {
					let call_index = call_index.clone();
					let release_slow = release_slow.clone();
					respond_with(move || {
						let event = if call_index.get() == 0 {
							let message = create_assistant_message(
								content(json!([
									{ "type": "toolCall", "id": "tool-1", "name": "slow", "arguments": { "value": "a" } },
									{ "type": "toolCall", "id": "tool-2", "name": "fast", "arguments": { "value": "b" } },
								])),
								StopReason::ToolUse,
							);
							pi_host::spawn_local(async move {
								let _ = pi_host::sleep(20.0, None).await;
								let release = release_slow.borrow_mut().take();
								if let Some(release) = release {
									let _ = release.send(());
								}
							});
							done(StopReason::ToolUse, message)
						} else {
							let message = create_assistant_message(
								content(json!([{ "type": "text", "text": "done" }])),
								StopReason::Stop,
							);
							done(StopReason::Stop, message)
						};
						call_index.set(call_index.get() + 1);
						event
					})
				})),
			);

			drain(&stream).await;

			// Fast tool should NOT run before slow tool finishes
			assert_eq!(execution_order.borrow()[0], "slow:a");
			assert!(execution_order.borrow().contains(&"fast:b".to_string()));
		});
	}

	#[test]
	fn should_allow_parallel_execution_when_all_tools_have_execution_mode_parallel() {
		pi_host::testing::run(|_host| async move {
			let first_resolved = Rc::new(Cell::new(false));
			let parallel_observed = Rc::new(Cell::new(false));
			let (release_first, first_done) = oneshot::channel::<()>();
			let release_first = Rc::new(RefCell::new(Some(release_first)));

			let mut tool = waiting_tool(
				"echo",
				"Echo",
				"Echo tool",
				"echoed: ",
				first_done,
				first_resolved.clone(),
				parallel_observed.clone(),
			);
			tool.execution_mode = Some(ToolExecutionMode::Parallel);

			let context = AgentContext {
				messages: vec![],
				tools: Some(vec![tool]),
			};

			let user_prompt: AgentMessage = create_user_message("echo both").into();
			let config = AgentLoopConfig::new(create_model(), identity_converter());

			let stream = agent_loop(
				vec![user_prompt],
				context,
				config,
				None,
				Some(two_tool_calls_then_done("echo", ["first", "second"], release_first)),
			);

			drain(&stream).await;

			// With executionMode=parallel, second tool should start before first finishes
			assert!(parallel_observed.get());
		});
	}

	#[test]
	fn runs_finish_turn_after_tool_result_messages_and_before_turn_end() {
		pi_host::testing::run(|_host| async move {
			let tool = agent_tool(
				"echo",
				"Echo",
				"Echo tool",
				value_schema(),
				|_tool_call_id, params, _, _| {
					Box::pin(async move {
						Ok(tool_result(json!({
							"content": [{ "type": "text", "text": params["value"] }],
							"details": { "value": params["value"] },
							"terminate": true,
						})))
					})
				},
			);
			let ordering: Rc<RefCell<Vec<String>>> = Rc::new(RefCell::new(Vec::new()));
			let finish_ordering = ordering.clone();
			let config = AgentLoopConfig {
				finish_turn: Some(Rc::new(move |turn: AgentTurnContext, _signal| {
					finish_ordering.borrow_mut().push("finishTurn".into());
					assert_eq!(turn.tool_results.len(), 1);
					assert_eq!(turn.context.messages.last().map(AgentMessage::role), Some("toolResult"));
					Box::pin(async { Ok(None) })
				})),
				..AgentLoopConfig::new(create_model(), identity_converter())
			};

			let event_ordering = ordering.clone();
			run_agent_loop(
				vec![create_user_message("echo").into()],
				AgentContext {
					messages: vec![],
					tools: Some(vec![tool]),
				},
				config,
				sink(move |event| {
					if let AgentEvent::MessageEnd { message } = &event {
						event_ordering
							.borrow_mut()
							.push(format!("message_end:{}", message.role()));
					}
					if let AgentEvent::TurnEnd { .. } = event {
						event_ordering.borrow_mut().push("turn_end".into());
					}
				}),
				None,
				Some(as_stream_fn(|_, _, _| {
					respond_with(|| {
						done(
							StopReason::ToolUse,
							create_assistant_message(
								content(
									json!([{ "type": "toolCall", "id": "tool-1", "name": "echo", "arguments": { "value": "hello" } }]),
								),
								StopReason::ToolUse,
							),
						)
					})
				})),
			)
			.await
			.expect("runAgentLoop");

			let ordering = ordering.borrow();
			assert_eq!(
				ordering[ordering.len() - 3..],
				["message_end:toolResult", "finishTurn", "turn_end"]
			);
		});
	}

	fn runs_finish_turn_for_an_assistant_before_turn_end_without_changing_the_hard_exit(reason: StopReason) {
		pi_host::testing::run(|_host| async move {
			let ordering: Rc<RefCell<Vec<String>>> = Rc::new(RefCell::new(Vec::new()));
			let provider_calls = Rc::new(Cell::new(0));
			let steering_polls = Rc::new(Cell::new(0));
			let follow_up_polls = Rc::new(Cell::new(0));
			let finish_ordering = ordering.clone();
			let steering = steering_polls.clone();
			let follow_up = follow_up_polls.clone();
			let event_ordering = ordering.clone();
			let provider = provider_calls.clone();
			run_agent_loop(
				vec![create_user_message("run").into()],
				AgentContext {
					messages: vec![],
					tools: Some(vec![]),
				},
				AgentLoopConfig {
					finish_turn: Some(Rc::new(move |turn: AgentTurnContext, _signal| {
						assert_eq!(turn.message.stop_reason, reason);
						finish_ordering.borrow_mut().push("finishTurn".into());
						Box::pin(async { Ok(Some(AgentTurnDecision::Continue)) })
					})),
					get_steering_messages: Some(Rc::new(move || {
						steering.set(steering.get() + 1);
						Box::pin(async { Ok(vec![]) })
					})),
					get_follow_up_messages: Some(Rc::new(move || {
						follow_up.set(follow_up.get() + 1);
						Box::pin(async { Ok(vec![create_user_message("queued").into()]) })
					})),
					..AgentLoopConfig::new(create_model(), identity_converter())
				},
				sink(move |event| {
					if let AgentEvent::TurnEnd { .. } = event {
						event_ordering.borrow_mut().push("turn_end".into());
					}
				}),
				None,
				Some(as_stream_fn(move |_, _, _| {
					provider.set(provider.get() + 1);
					respond_with(move || {
						let reason_text = serde_json::to_value(reason).expect("stop reason serializes");
						AssistantMessageEvent::Error {
							reason,
							error: Rc::new(RefCell::new(AssistantMessage {
								error_message: reason_text.as_str().map(str::to_string),
								..create_assistant_message(vec![], reason)
							})),
						}
					})
				})),
			)
			.await
			.expect("runAgentLoop");

			assert_eq!(*ordering.borrow(), vec!["finishTurn", "turn_end"]);
			assert_eq!(provider_calls.get(), 1);
			assert_eq!(steering_polls.get(), 1);
			assert_eq!(follow_up_polls.get(), 0);
		});
	}

	// it.each(["error", "aborted"])
	#[test]
	fn runs_finish_turn_for_a_error_assistant_before_turn_end_without_changing_the_hard_exit() {
		runs_finish_turn_for_an_assistant_before_turn_end_without_changing_the_hard_exit(StopReason::Error);
	}

	#[test]
	fn runs_finish_turn_for_a_aborted_assistant_before_turn_end_without_changing_the_hard_exit() {
		runs_finish_turn_for_an_assistant_before_turn_end_without_changing_the_hard_exit(StopReason::Aborted);
	}

	/// The `noop` tool: `obj({})` parameters, `details: undefined`.
	fn noop_tool() -> AgentTool {
		agent_tool("noop", "Noop", "Noop tool", empty_schema(), |_, _, _, _| {
			Box::pin(async { Ok(tool_result(json!({ "content": [{ "type": "text", "text": "done" }] }))) })
		})
	}

	#[test]
	fn action_end_skips_queue_polling_and_next_turn_preparation() {
		pi_host::testing::run(|_host| async move {
			let tool = noop_tool();
			let provider_calls = Rc::new(Cell::new(0));
			let steering_polls = Rc::new(Cell::new(0));
			let follow_up_polls = Rc::new(Cell::new(0));
			let prepare_next_turn_calls = Rc::new(Cell::new(0));
			let (steering, follow_up, prepare) = (
				steering_polls.clone(),
				follow_up_polls.clone(),
				prepare_next_turn_calls.clone(),
			);
			let config = AgentLoopConfig {
				finish_turn: Some(Rc::new(|_turn: AgentTurnContext, _signal| {
					Box::pin(async { Ok(Some(AgentTurnDecision::End)) })
				})),
				prepare_next_turn: Some(Rc::new(move |_turn: AgentTurnContext| {
					prepare.set(prepare.get() + 1);
					Box::pin(async { Ok(None) })
				})),
				get_steering_messages: Some(Rc::new(move || {
					steering.set(steering.get() + 1);
					Box::pin(async { Ok(vec![]) })
				})),
				get_follow_up_messages: Some(Rc::new(move || {
					follow_up.set(follow_up.get() + 1);
					Box::pin(async { Ok(vec![create_user_message("queued").into()]) })
				})),
				..AgentLoopConfig::new(create_model(), identity_converter())
			};

			let provider = provider_calls.clone();
			let stream = agent_loop(
				vec![create_user_message("run").into()],
				AgentContext {
					messages: vec![],
					tools: Some(vec![tool]),
				},
				config,
				None,
				Some(as_stream_fn(move |_, _, _| {
					provider.set(provider.get() + 1);
					respond_with(|| {
						done(
							StopReason::ToolUse,
							create_assistant_message(
								content(
									json!([{ "type": "toolCall", "id": "tool-1", "name": "noop", "arguments": {} }]),
								),
								StopReason::ToolUse,
							),
						)
					})
				})),
			);
			stream.result().await;

			assert_eq!(provider_calls.get(), 1);
			assert_eq!(steering_polls.get(), 1);
			assert_eq!(follow_up_polls.get(), 0);
			assert_eq!(prepare_next_turn_calls.get(), 0);
		});
	}

	#[test]
	fn makes_exactly_one_context_only_request_when_no_natural_request_satisfies_continuation() {
		pi_host::testing::run(|_host| async move {
			let provider_calls = Rc::new(Cell::new(0));
			let finish_calls = Rc::new(Cell::new(0));
			let (finish, provider) = (finish_calls.clone(), provider_calls.clone());
			let stream = agent_loop(
				vec![create_user_message("run").into()],
				AgentContext {
					messages: vec![],
					tools: Some(vec![]),
				},
				AgentLoopConfig {
					finish_turn: Some(Rc::new(move |_turn: AgentTurnContext, _signal| {
						finish.set(finish.get() + 1);
						let decision = (finish.get() == 1).then_some(AgentTurnDecision::Continue);
						Box::pin(async move { Ok(decision) })
					})),
					..AgentLoopConfig::new(create_model(), identity_converter())
				},
				None,
				Some(as_stream_fn(move |_, _, _| {
					provider.set(provider.get() + 1);
					let provider = provider.clone();
					respond_with(move || {
						let text = format!("response {}", provider.get());
						done(
							StopReason::Stop,
							create_assistant_message(
								content(json!([{ "type": "text", "text": text }])),
								StopReason::Stop,
							),
						)
					})
				})),
			);
			stream.result().await;

			assert_eq!(provider_calls.get(), 2);
			assert_eq!(finish_calls.get(), 2);
		});
	}

	#[test]
	fn lets_a_natural_tool_result_request_satisfy_continuation() {
		pi_host::testing::run(|_host| async move {
			let tool = noop_tool();
			let provider_calls = Rc::new(Cell::new(0));
			let finish_calls = Rc::new(Cell::new(0));
			let finish = finish_calls.clone();
			let config = AgentLoopConfig {
				finish_turn: Some(Rc::new(move |_turn: AgentTurnContext, _signal| {
					finish.set(finish.get() + 1);
					let decision = (finish.get() == 1).then_some(AgentTurnDecision::Continue);
					Box::pin(async move { Ok(decision) })
				})),
				..AgentLoopConfig::new(create_model(), identity_converter())
			};

			let provider = provider_calls.clone();
			let stream = agent_loop(
				vec![create_user_message("run").into()],
				AgentContext {
					messages: vec![],
					tools: Some(vec![tool]),
				},
				config,
				None,
				Some(as_stream_fn(move |_, _, _| {
					provider.set(provider.get() + 1);
					let provider = provider.clone();
					respond_with(move || {
						let message = if provider.get() == 1 {
							create_assistant_message(
								content(
									json!([{ "type": "toolCall", "id": "tool-1", "name": "noop", "arguments": {} }]),
								),
								StopReason::ToolUse,
							)
						} else {
							create_assistant_message(
								content(json!([{ "type": "text", "text": "done" }])),
								StopReason::Stop,
							)
						};
						done(
							if provider.get() == 1 {
								StopReason::ToolUse
							} else {
								StopReason::Stop
							},
							message,
						)
					})
				})),
			);
			stream.result().await;

			assert_eq!(provider_calls.get(), 2);
			assert_eq!(finish_calls.get(), 2);
		});
	}

	fn lets_a_natural_queued_request_satisfy_continuation(queue_kind: &'static str) {
		pi_host::testing::run(|_host| async move {
			let queued_message: AgentMessage = create_user_message(queue_kind).into();
			let provider_calls = Rc::new(Cell::new(0));
			let finish_calls = Rc::new(Cell::new(0));
			let steering_polls = Rc::new(Cell::new(0));
			let follow_up_delivered = Rc::new(Cell::new(false));
			let second_request_users: Rc<RefCell<Vec<String>>> = Rc::new(RefCell::new(Vec::new()));
			let finish = finish_calls.clone();
			let steering = steering_polls.clone();
			let (steering_message, follow_up_message) = (queued_message.clone(), queued_message.clone());
			let config = AgentLoopConfig {
				finish_turn: Some(Rc::new(move |_turn: AgentTurnContext, _signal| {
					finish.set(finish.get() + 1);
					let decision = (finish.get() == 1).then_some(AgentTurnDecision::Continue);
					Box::pin(async move { Ok(decision) })
				})),
				get_steering_messages: Some(Rc::new(move || {
					steering.set(steering.get() + 1);
					let messages = if queue_kind == "steering" && steering.get() == 2 {
						vec![steering_message.clone()]
					} else {
						vec![]
					};
					Box::pin(async move { Ok(messages) })
				})),
				get_follow_up_messages: Some(Rc::new(move || {
					let messages = if queue_kind != "follow-up" || follow_up_delivered.get() {
						vec![]
					} else {
						follow_up_delivered.set(true);
						vec![follow_up_message.clone()]
					};
					Box::pin(async move { Ok(messages) })
				})),
				..AgentLoopConfig::new(create_model(), identity_converter())
			};

			let (provider, users) = (provider_calls.clone(), second_request_users.clone());
			let stream = agent_loop(
				vec![create_user_message("run").into()],
				AgentContext {
					messages: vec![],
					tools: Some(vec![]),
				},
				config,
				None,
				Some(as_stream_fn(move |_model, context, _| {
					provider.set(provider.get() + 1);
					if provider.get() == 2 {
						users
							.borrow_mut()
							.extend(context.messages.iter().filter_map(user_text).map(str::to_string));
					}
					respond_with(|| {
						done(
							StopReason::Stop,
							create_assistant_message(
								content(json!([{ "type": "text", "text": "done" }])),
								StopReason::Stop,
							),
						)
					})
				})),
			);
			stream.result().await;

			assert_eq!(provider_calls.get(), 2);
			assert_eq!(finish_calls.get(), 2);
			assert!(second_request_users.borrow().contains(&queue_kind.to_string()));
		});
	}

	// it.each(["steering", "follow-up"])
	#[test]
	fn lets_a_natural_steering_request_satisfy_continuation() {
		lets_a_natural_queued_request_satisfy_continuation("steering");
	}

	#[test]
	fn lets_a_natural_follow_up_request_satisfy_continuation() {
		lets_a_natural_queued_request_satisfy_continuation("follow-up");
	}

	#[test]
	fn prepares_the_initial_request_after_pending_messages_and_can_replace_request_state() {
		pi_host::testing::run(|_host| async move {
			let mut replacement_model = create_model();
			replacement_model.id = "replacement".into();
			replacement_model.name = "replacement".into();
			let canonical_message = create_user_message("canonical projection");
			let steering_message: AgentMessage = create_user_message("steering").into();
			let completed_messages: Rc<RefCell<Vec<AgentMessage>>> = Rc::new(RefCell::new(Vec::new()));
			let steering_delivered = Rc::new(Cell::new(false));
			let prepare_calls = Rc::new(Cell::new(0));
			let (delivered_steering, expected_steering) = (steering_message.clone(), steering_message.clone());
			let (completed, prepared) = (completed_messages.clone(), prepare_calls.clone());
			let (replacement, canonical) = (replacement_model.clone(), canonical_message.clone());
			let config = AgentLoopConfig {
				get_steering_messages: Some(Rc::new(move || {
					let messages = if steering_delivered.get() {
						vec![]
					} else {
						steering_delivered.set(true);
						vec![delivered_steering.clone()]
					};
					Box::pin(async move { Ok(messages) })
				})),
				prepare_request: Some(Rc::new(move |request: PrepareRequestContext, _signal| {
					prepared.set(prepared.get() + 1);
					assert!(completed.borrow().contains(&expected_steering));
					assert!(request.context.messages.contains(&expected_steering));
					let update = AgentRequestUpdate {
						context: Some(AgentContext {
							messages: vec![canonical.clone().into()],
							..request.context
						}),
						model: Some(replacement.clone()),
						thinking_level: Some(ThinkingLevel::High),
					};
					Box::pin(async move { Ok(Some(update)) })
				})),
				..AgentLoopConfig::new(create_model(), identity_converter())
			};

			let completed = completed_messages.clone();
			run_agent_loop(
				vec![create_user_message("prompt").into()],
				AgentContext {
					messages: vec![],
					tools: Some(vec![]),
				},
				config,
				sink(move |event| {
					if let AgentEvent::MessageEnd { message } = event {
						completed.borrow_mut().push(message);
					}
				}),
				None,
				Some(as_stream_fn(move |model, context, options| {
					assert_eq!(model, &replacement_model);
					assert_eq!(context.messages, vec![Message::User(canonical_message.clone())]);
					assert_eq!(
						options.as_ref().and_then(|options| options.reasoning),
						Some(ReasoningLevel::High)
					);
					respond_with(|| {
						done(
							StopReason::Stop,
							create_assistant_message(
								content(json!([{ "type": "text", "text": "done" }])),
								StopReason::Stop,
							),
						)
					})
				})),
			)
			.await
			.expect("runAgentLoop");

			assert_eq!(prepare_calls.get(), 1);
		});
	}

	#[test]
	fn does_not_poll_steering_after_prepare_request() {
		pi_host::testing::run(|_host| async move {
			let queued: Rc<RefCell<Vec<AgentMessage>>> = Rc::new(RefCell::new(Vec::new()));
			let late_steering = create_user_message("late steering");
			let request_included_steering: Rc<RefCell<Vec<bool>>> = Rc::new(RefCell::new(Vec::new()));
			let request_preparations = Rc::new(Cell::new(0));
			let steering_polls = Rc::new(Cell::new(0));
			let (polled, steering) = (queued.clone(), steering_polls.clone());
			let (pushed, preparations, late) = (queued.clone(), request_preparations.clone(), late_steering.clone());
			let config = AgentLoopConfig {
				get_steering_messages: Some(Rc::new(move || {
					steering.set(steering.get() + 1);
					let messages = std::mem::take(&mut *polled.borrow_mut());
					Box::pin(async move { Ok(messages) })
				})),
				prepare_request: Some(Rc::new(move |_request: PrepareRequestContext, _signal| {
					preparations.set(preparations.get() + 1);
					if preparations.get() == 1 {
						pushed.borrow_mut().push(late.clone().into());
					}
					Box::pin(async { Ok(None) })
				})),
				..AgentLoopConfig::new(create_model(), identity_converter())
			};

			let included = request_included_steering.clone();
			let stream = agent_loop(
				vec![create_user_message("run").into()],
				AgentContext {
					messages: vec![],
					tools: Some(vec![]),
				},
				config,
				None,
				Some(as_stream_fn(move |_model, context, _| {
					included
						.borrow_mut()
						.push(context.messages.contains(&Message::User(late_steering.clone())));
					respond_with(|| {
						done(
							StopReason::Stop,
							create_assistant_message(
								content(json!([{ "type": "text", "text": "done" }])),
								StopReason::Stop,
							),
						)
					})
				})),
			);
			stream.result().await;

			assert_eq!(*request_included_steering.borrow(), vec![false, true]);
			assert_eq!(request_preparations.get(), 2);
			// Startup, post-turn delivery, then the final natural-stop check.
			assert_eq!(steering_polls.get(), 3);
		});
	}

	#[test]
	fn should_use_prepare_next_turn_snapshot_before_continuing() {
		pi_host::testing::run(|_host| async move {
			let tool = agent_tool(
				"echo",
				"Echo",
				"Echo tool",
				value_schema(),
				|_tool_call_id, params, _, _| {
					Box::pin(async move {
						let value = params["value"].as_str().unwrap_or_default().to_string();
						Ok(tool_result(json!({
							"content": [{ "type": "text", "text": format!("echoed: {value}") }],
							"details": { "value": value },
						})))
					})
				},
			);
			let context = AgentContext {
				messages: vec![],
				tools: Some(vec![tool]),
			};
			let converted_second_turn_has_update = Rc::new(Cell::new(false));
			let prepare_calls = Rc::new(Cell::new(0));
			let prepared = Rc::new(Cell::new(false));
			let prepare = prepare_calls.clone();
			let config = AgentLoopConfig {
				prepare_next_turn: Some(Rc::new(move |turn: AgentTurnContext| {
					prepare.set(prepare.get() + 1);
					let update = if prepared.get() {
						None
					} else {
						prepared.set(true);
						Some(AgentLoopTurnUpdate {
							context: Some(AgentContext {
								messages: turn.context.messages.clone(),
								tools: turn.context.tools.clone(),
							}),
							messages: Some(vec![
								SystemMessage {
									content: SystemMessageContent::Text("updated guidance".into()),
									timestamp: 1.0,
									..SystemMessage::default()
								}
								.into(),
							]),
							..AgentLoopTurnUpdate::default()
						})
					};
					Box::pin(async move { Ok(update) })
				})),
				..AgentLoopConfig::new(create_model(), identity_converter())
			};

			let llm_calls = Rc::new(Cell::new(0));
			let (calls, has_update) = (llm_calls.clone(), converted_second_turn_has_update.clone());
			let stream = agent_loop(
				vec![create_user_message("echo something").into()],
				context,
				config,
				None,
				Some(as_stream_fn(move |_model, ctx, _| {
					calls.set(calls.get() + 1);
					if calls.get() == 2 {
						has_update.set(ctx.messages.iter().any(|message| {
							matches!(
								message,
								Message::System(SystemMessage { content: SystemMessageContent::Text(text), .. })
									if text == "updated guidance"
							)
						}));
					}
					let calls = calls.clone();
					respond_with(move || {
						if calls.get() == 1 {
							done(
								StopReason::ToolUse,
								create_assistant_message(
									content(
										json!([{ "type": "toolCall", "id": "tool-1", "name": "echo", "arguments": { "value": "hello" } }]),
									),
									StopReason::ToolUse,
								),
							)
						} else {
							done(
								StopReason::Stop,
								create_assistant_message(
									content(json!([{ "type": "text", "text": "done" }])),
									StopReason::Stop,
								),
							)
						}
					})
				})),
			);

			drain(&stream).await;

			assert_eq!(llm_calls.get(), 2);
			assert_eq!(prepare_calls.get(), 1);
			assert!(converted_second_turn_has_update.get());
		});
	}

	#[test]
	fn picks_up_steering_queued_during_prepare_next_turn_before_the_next_request() {
		pi_host::testing::run(|_host| async move {
			let tool = noop_tool();
			let queued: Rc<RefCell<Vec<AgentMessage>>> = Rc::new(RefCell::new(Vec::new()));
			let late_steering = create_user_message("late steering");
			let provider_calls = Rc::new(Cell::new(0));
			let second_request_included_steering = Rc::new(Cell::new(false));
			let (pushed, polled, late) = (queued.clone(), queued.clone(), late_steering.clone());
			let config = AgentLoopConfig {
				prepare_next_turn: Some(Rc::new(move |_turn: AgentTurnContext| {
					pushed.borrow_mut().push(late.clone().into());
					Box::pin(async { Ok(None) })
				})),
				get_steering_messages: Some(Rc::new(move || {
					let messages = std::mem::take(&mut *polled.borrow_mut());
					Box::pin(async move { Ok(messages) })
				})),
				..AgentLoopConfig::new(create_model(), identity_converter())
			};

			let (provider, included) = (provider_calls.clone(), second_request_included_steering.clone());
			let stream = agent_loop(
				vec![create_user_message("run").into()],
				AgentContext {
					messages: vec![],
					tools: Some(vec![tool]),
				},
				config,
				None,
				Some(as_stream_fn(move |_model, context, _| {
					provider.set(provider.get() + 1);
					if provider.get() == 2 {
						included.set(context.messages.contains(&Message::User(late_steering.clone())));
					}
					let provider = provider.clone();
					respond_with(move || {
						let message = if provider.get() == 1 {
							create_assistant_message(
								content(
									json!([{ "type": "toolCall", "id": "tool-1", "name": "noop", "arguments": {} }]),
								),
								StopReason::ToolUse,
							)
						} else {
							create_assistant_message(
								content(json!([{ "type": "text", "text": "done" }])),
								StopReason::Stop,
							)
						};
						done(
							if provider.get() == 1 {
								StopReason::ToolUse
							} else {
								StopReason::Stop
							},
							message,
						)
					})
				})),
			);
			stream.result().await;

			assert_eq!(provider_calls.get(), 2);
			assert!(second_request_included_steering.get());
		});
	}

	#[test]
	fn action_end_receives_finalized_turn_context_and_stops_before_queue_polling() {
		pi_host::testing::run(|_host| async move {
			let executed: Rc<RefCell<Vec<String>>> = Rc::new(RefCell::new(Vec::new()));
			let tool = {
				let executed = executed.clone();
				agent_tool(
					"echo",
					"Echo",
					"Echo tool",
					value_schema(),
					move |_tool_call_id, params, _, _| {
						let executed = executed.clone();
						Box::pin(async move {
							let value = params["value"].as_str().unwrap_or_default().to_string();
							executed.borrow_mut().push(value.clone());
							Ok(tool_result(json!({
								"content": [{ "type": "text", "text": format!("echoed: {value}") }],
								"details": { "value": value },
							})))
						})
					},
				)
			};

			let context = AgentContext {
				messages: vec![],
				tools: Some(vec![tool]),
			};

			let steering_polls = Rc::new(Cell::new(0));
			let follow_up_polls = Rc::new(Cell::new(0));
			let callback_tool_result_ids: Rc<RefCell<Vec<String>>> = Rc::new(RefCell::new(Vec::new()));
			let callback_context_roles: Rc<RefCell<Vec<String>>> = Rc::new(RefCell::new(Vec::new()));
			let (tool_result_ids, context_roles) = (callback_tool_result_ids.clone(), callback_context_roles.clone());
			let (steering, follow_up) = (steering_polls.clone(), follow_up_polls.clone());
			let config = AgentLoopConfig {
				finish_turn: Some(Rc::new(move |turn: AgentTurnContext, _signal| {
					let (tool_result_ids, context_roles) = (tool_result_ids.clone(), context_roles.clone());
					Box::pin(async move {
						assert_eq!(AgentMessage::from(turn.message).role(), "assistant");
						*tool_result_ids.borrow_mut() = turn
							.tool_results
							.iter()
							.map(|tool_result| tool_result.tool_call_id.clone())
							.collect();
						*context_roles.borrow_mut() = turn
							.context
							.messages
							.iter()
							.map(|message| message.role().to_string())
							.collect();
						Ok(Some(AgentTurnDecision::End))
					})
				})),
				get_steering_messages: Some(Rc::new(move || {
					steering.set(steering.get() + 1);
					Box::pin(async { Ok(vec![]) })
				})),
				get_follow_up_messages: Some(Rc::new(move || {
					follow_up.set(follow_up.get() + 1);
					Box::pin(async { Ok(vec![create_user_message("follow up should stay queued").into()]) })
				})),
				..AgentLoopConfig::new(create_model(), identity_converter())
			};

			let llm_calls = Rc::new(Cell::new(0));
			let calls = llm_calls.clone();
			let stream = agent_loop(
				vec![create_user_message("echo something").into()],
				context,
				config,
				None,
				Some(as_stream_fn(move |_, _, _| {
					calls.set(calls.get() + 1);
					let calls = calls.clone();
					respond_with(move || {
						if calls.get() == 1 {
							let message = create_assistant_message(
								content(
									json!([{ "type": "toolCall", "id": "tool-1", "name": "echo", "arguments": { "value": "hello" } }]),
								),
								StopReason::ToolUse,
							);
							done(StopReason::ToolUse, message)
						} else {
							done(
								StopReason::Stop,
								create_assistant_message(
									content(json!([{ "type": "text", "text": "should not run" }])),
									StopReason::Stop,
								),
							)
						}
					})
				})),
			);

			let events = drain(&stream).await;

			let messages = stream.result().await.expect("agent loop result");
			assert_eq!(llm_calls.get(), 1);
			assert_eq!(*executed.borrow(), vec!["hello"]);
			assert_eq!(steering_polls.get(), 1);
			assert_eq!(follow_up_polls.get(), 0);
			assert_eq!(*callback_tool_result_ids.borrow(), vec!["tool-1"]);
			assert_eq!(
				*callback_context_roles.borrow(),
				vec!["system", "user", "assistant", "toolResult"]
			);
			// The context declares no tools, so the loop announces the loadout with a system message.
			assert_eq!(roles(&messages), vec!["system", "user", "assistant", "toolResult"]);
			assert_eq!(
				events.iter().map(event_type).collect::<Vec<_>>(),
				vec![
					"agent_start",
					"turn_start",
					"message_start",
					"message_end",
					"message_start",
					"message_end",
					"message_start",
					"message_end",
					"tool_execution_start",
					"tool_execution_end",
					"message_start",
					"message_end",
					"turn_end",
					"agent_end",
				]
			);
		});
	}

	#[test]
	fn should_stop_after_a_tool_batch_when_every_tool_result_sets_terminate_true() {
		pi_host::testing::run(|_host| async move {
			let tool = agent_tool(
				"echo",
				"Echo",
				"Echo tool",
				value_schema(),
				|_tool_call_id, params, _, _| {
					Box::pin(async move {
						let value = params["value"].as_str().unwrap_or_default().to_string();
						Ok(tool_result(json!({
							"content": [{ "type": "text", "text": format!("echoed: {value}") }],
							"details": { "value": value },
							"terminate": true,
						})))
					})
				},
			);

			let context = AgentContext {
				messages: vec![],
				tools: Some(vec![tool]),
			};

			let config = AgentLoopConfig::new(create_model(), identity_converter());

			let llm_calls = Rc::new(Cell::new(0));
			let calls = llm_calls.clone();
			let stream = agent_loop(
				vec![create_user_message("echo something").into()],
				context,
				config,
				None,
				Some(as_stream_fn(move |_, _, _| {
					calls.set(calls.get() + 1);
					respond_with(|| {
						let message = create_assistant_message(
							content(
								json!([{ "type": "toolCall", "id": "tool-1", "name": "echo", "arguments": { "value": "hello" } }]),
							),
							StopReason::ToolUse,
						);
						done(StopReason::ToolUse, message)
					})
				})),
			);

			let events = drain(&stream).await;

			let messages = stream.result().await.expect("agent loop result");
			assert_eq!(llm_calls.get(), 1);
			assert_eq!(roles(&messages), vec!["system", "user", "assistant", "toolResult"]);
			assert_eq!(
				events
					.iter()
					.filter(|event| matches!(event, AgentEvent::TurnEnd { .. }))
					.count(),
				1
			);
		});
	}

	#[test]
	fn should_stop_after_a_blocked_tool_call_when_before_tool_call_sets_terminate_true() {
		pi_host::testing::run(|_host| async move {
			let executed = Rc::new(Cell::new(false));
			let tool = {
				let executed = executed.clone();
				agent_tool("echo", "Echo", "Echo tool", value_schema(), move |_, _, _, _| {
					executed.set(true);
					Box::pin(async {
						Ok(tool_result(json!({
							"content": [{ "type": "text", "text": "should not execute" }],
							"details": { "value": "unexpected" },
						})))
					})
				})
			};
			let context = AgentContext {
				messages: vec![],
				tools: Some(vec![tool]),
			};
			let config = AgentLoopConfig {
				before_tool_call: Some(Rc::new(|_hook: BeforeToolCallContext, _signal| {
					Box::pin(async {
						Ok(Some(BeforeToolCallResult {
							block: Some(true),
							reason: Some("Blocked by policy".into()),
							terminate: Some(true),
						}))
					})
				})),
				..AgentLoopConfig::new(create_model(), identity_converter())
			};

			let llm_calls = Rc::new(Cell::new(0));
			let calls = llm_calls.clone();
			let stream = agent_loop(
				vec![create_user_message("echo something").into()],
				context,
				config,
				None,
				Some(as_stream_fn(move |_, _, _| {
					calls.set(calls.get() + 1);
					let calls = calls.clone();
					respond_with(move || {
						let message = if calls.get() == 1 {
							create_assistant_message(
								content(
									json!([{ "type": "toolCall", "id": "tool-1", "name": "echo", "arguments": { "value": "hello" } }]),
								),
								StopReason::ToolUse,
							)
						} else {
							create_assistant_message(
								content(json!([{ "type": "text", "text": "should not run" }])),
								StopReason::Stop,
							)
						};
						done(
							if calls.get() == 1 {
								StopReason::ToolUse
							} else {
								StopReason::Stop
							},
							message,
						)
					})
				})),
			);

			drain(&stream).await;

			let messages = stream.result().await.expect("agent loop result");
			let tool_result = messages.iter().find_map(tool_result_message);
			assert!(!executed.get());
			assert_eq!(llm_calls.get(), 1);
			assert!(tool_result.is_some_and(|tool_result| tool_result.is_error));
			let tool_result_content =
				serde_json::to_value(tool_result.map(|tool_result| &tool_result.content)).expect("content serializes");
			assert!(
				tool_result_content
					.as_array()
					.is_some_and(|content| content.contains(&json!({ "type": "text", "text": "Blocked by policy" })))
			);
		});
	}

	#[test]
	fn should_continue_after_a_mixed_batch_with_one_terminating_blocked_call() {
		pi_host::testing::run(|_host| async move {
			let executed: Rc<RefCell<Vec<String>>> = Rc::new(RefCell::new(Vec::new()));
			let tool = {
				let executed = executed.clone();
				agent_tool(
					"echo",
					"Echo",
					"Echo tool",
					value_schema(),
					move |_tool_call_id, params, _, _| {
						let executed = executed.clone();
						Box::pin(async move {
							let value = params["value"].as_str().unwrap_or_default().to_string();
							executed.borrow_mut().push(value.clone());
							Ok(tool_result(json!({
								"content": [{ "type": "text", "text": format!("echoed: {value}") }],
								"details": { "value": value },
							})))
						})
					},
				)
			};
			let context = AgentContext {
				messages: vec![],
				tools: Some(vec![tool]),
			};
			let config = AgentLoopConfig {
				tool_execution: Some(ToolExecutionMode::Parallel),
				before_tool_call: Some(Rc::new(|hook: BeforeToolCallContext, _signal| {
					Box::pin(async move {
						let is_first = hook.args.borrow().get("value").and_then(Value::as_str) == Some("first");
						Ok(is_first.then(|| BeforeToolCallResult {
							block: Some(true),
							reason: Some("Blocked first".into()),
							terminate: Some(true),
						}))
					})
				})),
				..AgentLoopConfig::new(create_model(), identity_converter())
			};

			let llm_calls = Rc::new(Cell::new(0));
			let calls = llm_calls.clone();
			let stream = agent_loop(
				vec![create_user_message("echo both").into()],
				context,
				config,
				None,
				Some(as_stream_fn(move |_, _, _| {
					calls.set(calls.get() + 1);
					let calls = calls.clone();
					respond_with(move || {
						let message = if calls.get() == 1 {
							create_assistant_message(
								content(json!([
									{ "type": "toolCall", "id": "tool-1", "name": "echo", "arguments": { "value": "first" } },
									{ "type": "toolCall", "id": "tool-2", "name": "echo", "arguments": { "value": "second" } },
								])),
								StopReason::ToolUse,
							)
						} else {
							create_assistant_message(
								content(json!([{ "type": "text", "text": "done" }])),
								StopReason::Stop,
							)
						};
						done(
							if calls.get() == 1 {
								StopReason::ToolUse
							} else {
								StopReason::Stop
							},
							message,
						)
					})
				})),
			);

			drain(&stream).await;

			assert_eq!(*executed.borrow(), vec!["second"]);
			assert_eq!(llm_calls.get(), 2);
		});
	}

	#[test]
	fn should_continue_after_parallel_tool_calls_when_not_all_tool_results_terminate() {
		pi_host::testing::run(|_host| async move {
			let tool = agent_tool(
				"echo",
				"Echo",
				"Echo tool",
				value_schema(),
				|_tool_call_id, params, _, _| {
					Box::pin(async move {
						let value = params["value"].as_str().unwrap_or_default().to_string();
						Ok(tool_result(json!({
							"content": [{ "type": "text", "text": format!("echoed: {value}") }],
							"details": { "value": value },
							"terminate": value == "first",
						})))
					})
				},
			);

			let context = AgentContext {
				messages: vec![],
				tools: Some(vec![tool]),
			};

			let config = AgentLoopConfig {
				tool_execution: Some(ToolExecutionMode::Parallel),
				..AgentLoopConfig::new(create_model(), identity_converter())
			};

			let call_index = Rc::new(Cell::new(0));
			let counter = call_index.clone();
			let stream = agent_loop(
				vec![create_user_message("echo both").into()],
				context,
				config,
				None,
				Some(as_stream_fn(move |_, _, _| {
					let call_index = counter.clone();
					respond_with(move || {
						let event = if call_index.get() == 0 {
							let message = create_assistant_message(
								content(json!([
									{ "type": "toolCall", "id": "tool-1", "name": "echo", "arguments": { "value": "first" } },
									{ "type": "toolCall", "id": "tool-2", "name": "echo", "arguments": { "value": "second" } },
								])),
								StopReason::ToolUse,
							);
							done(StopReason::ToolUse, message)
						} else {
							let message = create_assistant_message(
								content(json!([{ "type": "text", "text": "done" }])),
								StopReason::Stop,
							);
							done(StopReason::Stop, message)
						};
						call_index.set(call_index.get() + 1);
						event
					})
				})),
			);

			drain(&stream).await;

			let messages = stream.result().await.expect("agent loop result");
			assert_eq!(call_index.get(), 2);
			assert_eq!(
				roles(&messages),
				vec!["system", "user", "assistant", "toolResult", "toolResult", "assistant"]
			);
		});
	}

	#[test]
	fn should_allow_after_tool_call_to_mark_a_tool_batch_as_terminating() {
		pi_host::testing::run(|_host| async move {
			let tool = agent_tool(
				"echo",
				"Echo",
				"Echo tool",
				value_schema(),
				|_tool_call_id, params, _, _| {
					Box::pin(async move {
						let value = params["value"].as_str().unwrap_or_default().to_string();
						Ok(tool_result(json!({
							"content": [{ "type": "text", "text": format!("echoed: {value}") }],
							"details": { "value": value },
						})))
					})
				},
			);

			let context = AgentContext {
				messages: vec![],
				tools: Some(vec![tool]),
			};

			let config = AgentLoopConfig {
				after_tool_call: Some(Rc::new(|_hook: AfterToolCallContext, _signal| {
					Box::pin(async {
						Ok(Some(AfterToolCallResult {
							terminate: Some(true),
							..AfterToolCallResult::default()
						}))
					})
				})),
				..AgentLoopConfig::new(create_model(), identity_converter())
			};

			let llm_calls = Rc::new(Cell::new(0));
			let calls = llm_calls.clone();
			let stream = agent_loop(
				vec![create_user_message("echo something").into()],
				context,
				config,
				None,
				Some(as_stream_fn(move |_, _, _| {
					calls.set(calls.get() + 1);
					respond_with(|| {
						let message = create_assistant_message(
							content(
								json!([{ "type": "toolCall", "id": "tool-1", "name": "echo", "arguments": { "value": "hello" } }]),
							),
							StopReason::ToolUse,
						);
						done(StopReason::ToolUse, message)
					})
				})),
			);

			drain(&stream).await;

			assert_eq!(llm_calls.get(), 1);
		});
	}
}

mod agent_loop_continue_with_agent_message {
	use super::*;

	#[test]
	fn should_throw_when_context_has_no_messages() {
		let context = AgentContext {
			messages: vec![],
			tools: Some(vec![]),
		};

		let config = AgentLoopConfig::new(create_model(), identity_converter());

		let Err(error) = agent_loop_continue(
			context,
			config,
			None,
			Some(as_stream_fn(|_, _, _| {
				Box::pin(async { Err(PiError::new("Unexpected stream call")) })
			})),
		) else {
			panic!("expected agentLoopContinue to throw");
		};
		assert!(error.message.contains("Cannot continue: no messages in context"));
	}

	#[test]
	fn should_continue_from_existing_context_without_emitting_user_message_events() {
		pi_host::testing::run(|_host| async move {
			let user_message: AgentMessage = create_user_message("Hello").into();

			let context = AgentContext {
				messages: vec![user_message],
				tools: Some(vec![]),
			};

			let config = AgentLoopConfig::new(create_model(), identity_converter());

			let stream_fn = as_stream_fn(|_, _, _| {
				respond_with(|| {
					let message = create_assistant_message(
						content(json!([{ "type": "text", "text": "Response" }])),
						StopReason::Stop,
					);
					done(StopReason::Stop, message)
				})
			});

			let stream = agent_loop_continue(context, config, None, Some(stream_fn)).expect("agentLoopContinue");

			let events = drain(&stream).await;

			let messages = stream.result().await.expect("agent loop result");

			// Should only return the new assistant message (not the existing user message)
			assert_eq!(messages.len(), 1);
			assert_eq!(messages[0].role(), "assistant");

			// Should NOT have user message events (that's the key difference from agentLoop)
			let message_end_events: Vec<&AgentMessage> = events
				.iter()
				.filter_map(|event| match event {
					AgentEvent::MessageEnd { message } => Some(message),
					_ => None,
				})
				.collect();
			assert_eq!(message_end_events.len(), 1);
			assert_eq!(message_end_events[0].role(), "assistant");
		});
	}

	#[test]
	fn should_allow_custom_message_types_as_last_message_caller_responsibility() {
		pi_host::testing::run(|_host| async move {
			// Custom message that will be converted to user message by convertToLlm
			let custom_message = json!({
				"role": "custom",
				"text": "Hook content",
				"timestamp": pi_host::now_ms(),
			});

			let context = AgentContext {
				messages: vec![AgentMessage::Custom(custom_message)],
				tools: Some(vec![]),
			};

			let config = AgentLoopConfig::new(
				create_model(),
				Rc::new(|messages: Vec<AgentMessage>| {
					// Convert custom to user message
					let converted: Vec<Message> = messages
						.into_iter()
						.map(|message| match message {
							AgentMessage::Custom(custom) if custom["role"] == "custom" => Message::User(UserMessage {
								content: UserMessageContent::Text(custom["text"].as_str().unwrap_or_default().into()),
								timestamp: custom["timestamp"].as_f64().unwrap_or_default(),
								..UserMessage::default()
							})
							.into(),
							message => message,
						})
						.filter_map(|message| match message {
							AgentMessage::Llm(
								message @ (Message::User(_) | Message::Assistant(_) | Message::ToolResult(_)),
							) => Some(message),
							_ => None,
						})
						.collect();
					Box::pin(async move { Ok(converted) })
				}),
			);

			let stream_fn = as_stream_fn(|_, _, _| {
				respond_with(|| {
					let message = create_assistant_message(
						content(json!([{ "type": "text", "text": "Response to custom message" }])),
						StopReason::Stop,
					);
					done(StopReason::Stop, message)
				})
			});

			// Should not throw - the custom message will be converted to user message
			let stream = agent_loop_continue(context, config, None, Some(stream_fn)).expect("agentLoopContinue");

			let _events = drain(&stream).await;

			let messages = stream.result().await.expect("agent loop result");
			assert_eq!(messages.len(), 1);
			assert_eq!(messages[0].role(), "assistant");
		});
	}
}

mod run_tool_call {
	use super::*;

	fn echo() -> AgentTool {
		let mut echo = agent_tool(
			"echo",
			"Echo",
			"Echo tool",
			value_schema(),
			|_tool_call_id, params, _signal, on_update| {
				if let Some(on_update) = &on_update {
					on_update(tool_result(
						json!({ "content": [{ "type": "text", "text": "partial" }], "details": {} }),
					));
				}
				Box::pin(async move {
					Ok(tool_result(json!({
						"content": [{ "type": "text", "text": params["value"] }],
						"details": {},
						"structuredContent": { "value": params["value"] },
					})))
				})
			},
		);
		echo.output_schema = Some(value_schema());
		echo
	}

	fn failing() -> AgentTool {
		agent_tool(
			"failing",
			"Failing",
			"Returns an error result",
			empty_schema(),
			|_, _, _, _| {
				Box::pin(async {
					Ok(tool_result(
						json!({ "content": [{ "type": "text", "text": "bad" }], "details": { "partial": true }, "isError": true }),
					))
				})
			},
		)
	}

	fn assistant_message() -> AssistantMessage {
		create_assistant_message(vec![], StopReason::Stop)
	}

	fn call(id: &str, name: &str, args: Value) -> AgentToolCall {
		tool_call_of(id, name, args)
	}

	fn outcome_json(outcome: &AgentToolCallOutcome) -> Value {
		json!({ "toolCall": outcome.tool_call, "result": outcome.result, "isError": outcome.is_error })
	}

	#[test]
	fn validates_runs_the_hooks_and_reports_failures_as_error_outcomes() {
		pi_host::testing::run(|_host| async move {
			let hook_calls: Rc<RefCell<Vec<String>>> = Rc::new(RefCell::new(Vec::new()));
			let updates: Rc<RefCell<Vec<AgentToolResult>>> = Rc::new(RefCell::new(Vec::new()));
			let (before_calls, after_calls, recorded) = (hook_calls.clone(), hook_calls.clone(), updates.clone());
			let options = RunToolCallOptions {
				base: ToolCallHooks {
					before_tool_call: Some(Rc::new(move |hook: BeforeToolCallContext, _signal| {
						before_calls.borrow_mut().push(format!("before {}", hook.tool_call.id));
						let blocked = hook.args.borrow().get("value").and_then(Value::as_str) == Some("blocked");
						Box::pin(async move {
							Ok(blocked.then(|| BeforeToolCallResult {
								block: Some(true),
								reason: Some("nope".into()),
								..BeforeToolCallResult::default()
							}))
						})
					})),
					after_tool_call: Some(Rc::new(move |hook: AfterToolCallContext, _signal| {
						after_calls.borrow_mut().push(format!("after {}", hook.tool_call.id));
						Box::pin(async { Ok(None) })
					})),
				},
				tools: vec![echo(), failing()],
				assistant_message: assistant_message(),
				context: AgentContext {
					messages: vec![],
					tools: None,
				},
				signal: None,
				on_update: Some(Rc::new(move |partial| {
					recorded.borrow_mut().push(partial);
					Box::pin(async { Ok(()) })
				})),
			};

			let outcome = run_tool_call(call("a", "echo", json!({ "value": "a" })), options.clone())
				.await
				.expect("runToolCall");
			assert_match_object(
				&outcome_json(&outcome),
				&json!({
					"toolCall": { "id": "a" },
					"result": { "structuredContent": { "value": "a" } },
					"isError": false,
				}),
			);
			let outcome = run_tool_call(
				call("b", "echo", json!({ "value": { "nested": true } })),
				options.clone(),
			)
			.await
			.expect("runToolCall");
			assert_match_object(&outcome_json(&outcome), &json!({ "isError": true }));
			let outcome = run_tool_call(call("c", "echo", json!({ "value": "blocked" })), options.clone())
				.await
				.expect("runToolCall");
			assert_match_object(
				&outcome_json(&outcome),
				&json!({
					"result": { "content": [{ "type": "text", "text": "nope" }] },
					"isError": true,
				}),
			);
			let outcome = run_tool_call(call("d", "missing", json!({})), options.clone())
				.await
				.expect("runToolCall");
			assert_match_object(
				&outcome_json(&outcome),
				&json!({
					"result": { "content": [{ "type": "text", "text": "Tool missing not found" }] },
					"isError": true,
				}),
			);
			// Error results keep their details.
			let outcome = run_tool_call(call("e", "failing", json!({})), options.clone())
				.await
				.expect("runToolCall");
			assert_match_object(
				&outcome_json(&outcome),
				&json!({
					"result": { "details": { "partial": true } },
					"isError": true,
				}),
			);
			assert_eq!(
				serde_json::to_value(&*updates.borrow()).expect("updates serialize"),
				json!([{ "content": [{ "type": "text", "text": "partial" }], "details": {} }])
			);
			// Validation failures and unknown tools never reach the hooks; blocked calls skip afterToolCall.
			assert_eq!(
				*hook_calls.borrow(),
				vec!["before a", "after a", "before c", "before e", "after e"]
			);
		});
	}

	#[test]
	fn lets_after_tool_call_replace_structured_content_and_drops_it_when_only_content_is_replaced() {
		pi_host::testing::run(|_host| async move {
			let redacted = json!([{ "type": "text", "text": "redacted" }]);
			let results: Vec<AfterToolCallResult> = [
				json!({ "content": redacted }),
				json!({ "structuredContent": { "value": "replaced" } }),
				json!({ "content": redacted, "structuredContent": { "value": "both" } }),
				json!({ "details": { "note": "kept" } }),
			]
			.into_iter()
			.map(|result| serde_json::from_value(result).expect("afterToolCall result literal"))
			.collect();
			let mut seen: Vec<Option<Value>> = Vec::new();
			for after_result in results {
				let outcome = run_tool_call(
					call("x", "echo", json!({ "value": "original" })),
					RunToolCallOptions {
						base: ToolCallHooks {
							before_tool_call: None,
							after_tool_call: Some(Rc::new(move |_hook: AfterToolCallContext, _signal| {
								let after_result = after_result.clone();
								Box::pin(async move { Ok(Some(after_result)) })
							})),
						},
						tools: vec![echo()],
						assistant_message: assistant_message(),
						context: AgentContext {
							messages: vec![],
							tools: None,
						},
						signal: None,
						on_update: None,
					},
				)
				.await
				.expect("runToolCall");
				seen.push(outcome.result.structured_content);
			}
			assert_eq!(
				seen,
				vec![
					None,
					Some(json!({ "value": "replaced" })),
					Some(json!({ "value": "both" })),
					Some(json!({ "value": "original" })),
				]
			);
		});
	}
}
