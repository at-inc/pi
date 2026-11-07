//! Port of `packages/agent/test/agent.test.ts`.
//!
//! `getModel("openai", "gpt-4o-mini")` and `getModel("google", "gemini-2.5-flash")` come from pi-ai's `compat`
//! module, which is still pending (shard w2-ai-auth); the tests build equivalent models with `test_model`. Model
//! identity (`toBe`) becomes equality. `Reflect.construct(Agent, [{}])` is `Agent::new(AgentOptions::default())`.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use futures::FutureExt;
use futures::channel::oneshot;
use futures::future::{LocalBoxFuture, Shared};
use pi_agent::{
	Agent, AgentEvent, AgentInitialState, AgentListener, AgentMessage, AgentOptions, AgentTool, AgentToolResult,
	AgentToolUpdateCallback, AgentTurnDecision, QueueMode, StreamFn, ThinkingLevel, set_default_stream_fn,
};
use pi_ai::types::{
	AssistantContent, AssistantMessage, AssistantMessageEvent, Message, Model, StopReason, SystemMessage,
	SystemMessageContent, TextContent, Tool, ToolCall, ToolReference, ToolResultMessage, TranscriptContext,
	UserContent, UserMessage, UserMessageContent,
};
use pi_ai::utils::event_stream::AssistantMessageEventStream;
use pi_ai::utils::identity::{ClientAttribution, ClientIdentity, PI_ATTRIBUTION, UserAgentMode, UserAgentModeName};
use pi_ai::utils::transcript::{get_current_system_message, to_tool_declaration};
use pi_host::testing::run;
use pi_host::{AbortSignal, PiError, PiResult};
use pi_schema::builders::{Property, obj};
use serde_json::{Value, json};

fn create_user_message(text: &str) -> UserMessage {
	UserMessage {
		content: UserMessageContent::Text(text.into()),
		timestamp: pi_host::now_ms(),
		..UserMessage::default()
	}
}

fn create_assistant_message(text: &str) -> AssistantMessage {
	AssistantMessage {
		content: vec![AssistantContent::Text(TextContent {
			text: text.into(),
			..TextContent::default()
		})],
		api: "openai-responses".into(),
		provider: "openai".into(),
		model: "mock".into(),
		stop_reason: StopReason::Stop,
		timestamp: pi_host::now_ms(),
		..AssistantMessage::default()
	}
}

fn empty_obj() -> Value {
	obj(Vec::<(&str, Property)>::new(), Value::Null)
}

fn text_result(text: &str) -> AgentToolResult {
	AgentToolResult {
		content: vec![UserContent::Text(TextContent {
			text: text.into(),
			..TextContent::default()
		})],
		details: Some(json!({})),
		..AgentToolResult::default()
	}
}

fn tool(name: &str, label: &str, description: &str, execute: pi_agent::AgentToolExecute) -> AgentTool {
	AgentTool {
		tool: Tool {
			name: name.into(),
			description: description.into(),
			parameters: empty_obj(),
			..Tool::default()
		},
		label: label.into(),
		prepare_arguments: None,
		output_schema: None,
		execute,
		replay: None,
		execution_mode: None,
	}
}

fn returning(result: AgentToolResult) -> pi_agent::AgentToolExecute {
	Rc::new(move |_id, _params, _signal, _on_update| {
		let result = result.clone();
		Box::pin(async move { Ok(result) })
	})
}

fn create_tool(name: &str) -> AgentTool {
	tool(name, name, &format!("{name} tool"), returning(text_result(name)))
}

fn tool_call(id: &str, name: &str) -> ToolCall {
	ToolCall {
		id: id.into(),
		name: name.into(),
		..ToolCall::default()
	}
}

fn create_assistant_tool_use_message(content: Vec<ToolCall>) -> AssistantMessage {
	AssistantMessage {
		content: content.into_iter().map(AssistantContent::ToolCall).collect(),
		api: "openai-responses".into(),
		provider: "openai".into(),
		model: "mock".into(),
		stop_reason: StopReason::ToolUse,
		timestamp: pi_host::now_ms(),
		..AssistantMessage::default()
	}
}

fn unused_stream_function() -> StreamFn {
	Rc::new(|_model, _context, _options| Box::pin(async { Err(PiError::new("Unexpected stream call")) }))
}

struct Deferred {
	promise: Shared<LocalBoxFuture<'static, ()>>,
	resolve: RefCell<Option<oneshot::Sender<()>>>,
}

impl Deferred {
	fn resolve(&self) {
		if let Some(resolve) = self.resolve.borrow_mut().take() {
			let _ = resolve.send(());
		}
	}
}

fn create_deferred() -> Rc<Deferred> {
	let (resolve, promise) = oneshot::channel::<()>();
	Rc::new(Deferred {
		promise: promise.map(|_| ()).boxed_local().shared(),
		resolve: RefCell::new(Some(resolve)),
	})
}

/// A model like `getModel(provider, id)` returns, for the identity checks of these tests.
fn test_model(provider: &str, id: &str, api: &str) -> Model {
	serde_json::from_value(json!({
		"id": id,
		"name": id,
		"api": api,
		"provider": provider,
		"baseUrl": "https://example.invalid",
		"reasoning": true,
		"input": ["text", "image"],
		"cost": { "input": 0.15, "output": 0.6, "cacheRead": 0.08, "cacheWrite": 0 },
		"contextWindow": 128000,
		"maxTokens": 16384,
	}))
	.unwrap()
}

/// `createAssistantMessageEventStream()` plus `queueMicrotask(() => stream.push(event))`.
fn push_later(event: AssistantMessageEvent) -> AssistantMessageEventStream {
	let stream = AssistantMessageEventStream::new();
	let output = stream.clone();
	pi_host::spawn_local(async move { output.push(event) });
	stream
}

fn done(message: AssistantMessage) -> AssistantMessageEvent {
	AssistantMessageEvent::Done {
		reason: message.stop_reason,
		message: Rc::new(RefCell::new(message)),
	}
}

fn responding(text: &'static str) -> StreamFn {
	Rc::new(move |_model, _context, _options| {
		let stream = push_later(done(create_assistant_message(text)));
		Box::pin(async move { Ok(stream) })
	})
}

/// A stream that starts, then answers `aborted` once `signal` aborts, checking every 5 ms.
fn abortable_stream(signal: Option<AbortSignal>) -> AssistantMessageEventStream {
	let stream = AssistantMessageEventStream::new();
	let output = stream.clone();
	pi_host::spawn_local(async move {
		output.push(AssistantMessageEvent::Start {
			partial: Rc::new(RefCell::new(create_assistant_message(""))),
		});
		loop {
			if signal.as_ref().is_some_and(AbortSignal::aborted) {
				output.push(AssistantMessageEvent::Error {
					reason: StopReason::Aborted,
					error: Rc::new(RefCell::new(create_assistant_message("Aborted"))),
				});
				return;
			}
			pi_host::sleep(5.0, None).await.unwrap();
		}
	});
	stream
}

fn listener(on_event: impl Fn(&AgentEvent, &AbortSignal) + 'static) -> AgentListener {
	Rc::new(move |event, signal| {
		on_event(&event, &signal);
		Box::pin(async { Ok(()) })
	})
}

fn event_type(event: &AgentEvent) -> String {
	serde_json::to_value(event).unwrap()["type"]
		.as_str()
		.unwrap()
		.to_string()
}

/// `agent.prompt(input)` without awaiting it: the run starts on the next tick and its result arrives on the receiver.
fn spawn_prompt(agent: &Agent, input: &'static str) -> oneshot::Receiver<PiResult<()>> {
	let (resolve, result) = oneshot::channel();
	let agent = agent.clone();
	pi_host::spawn_local(async move {
		let _ = resolve.send(agent.prompt(input).await);
	});
	result
}

/// The string contents of the user messages in a request, like the TS tests' `requests.push(...)`.
fn user_texts(context: &TranscriptContext) -> Vec<String> {
	context
		.messages
		.iter()
		.filter_map(|message| match message {
			Message::User(UserMessage {
				content: UserMessageContent::Text(text),
				..
			}) => Some(text.clone()),
			_ => None,
		})
		.collect()
}

fn recording_user_texts(requests: &Rc<RefCell<Vec<Vec<String>>>>) -> StreamFn {
	let requests = requests.clone();
	Rc::new(move |_model, context, _options| {
		requests.borrow_mut().push(user_texts(context));
		let stream = push_later(done(create_assistant_message("done")));
		Box::pin(async move { Ok(stream) })
	})
}

/// A value as `JSON.parse(JSON.stringify(value))` gives it, so `1.0` compares equal to `1` like in JS; object
/// equality ignores key order like `toEqual`.
fn js_json(value: &impl serde::Serialize) -> Value {
	serde_json::from_str(&pi_host::js::json_stringify(value)).unwrap()
}

fn agent_with(options: AgentOptions) -> Agent {
	Agent::new(options).unwrap()
}

fn system_message(message: &AgentMessage) -> &SystemMessage {
	match message {
		AgentMessage::Llm(Message::System(message)) => message,
		other => panic!("expected initial system message, got {other:?}"),
	}
}

fn tool_names(tools: &Option<Vec<Tool>>) -> Vec<String> {
	tools.iter().flatten().map(|tool| tool.name.clone()).collect()
}

mod agent {
	use super::*;

	#[test]
	fn uses_the_configured_default_when_a_legacy_caller_omits_stream_fn() {
		run(|_host| async {
			let calls = Rc::new(Cell::new(0));
			let counter = calls.clone();
			set_default_stream_fn(Some(Rc::new(move |_model, _context, _options| {
				counter.set(counter.get() + 1);
				let stream = push_later(done(create_assistant_message("fallback")));
				Box::pin(async move { Ok(stream) })
			})));

			let agent = Agent::new(AgentOptions::default()).unwrap();
			agent.prompt("Hello").await.unwrap();
			assert_eq!(calls.get(), 1);
			set_default_stream_fn(None);
		});
	}

	#[test]
	fn should_create_an_agent_instance_with_default_state() {
		run(|_host| async {
			let agent = agent_with(AgentOptions {
				stream_fn: Some(unused_stream_function()),
				..AgentOptions::default()
			});

			let state = agent.state();
			assert_eq!(state.model.id, "unknown");
			assert_eq!(state.thinking_level, ThinkingLevel::Off);
			assert!(state.tools().is_empty());
			assert!(state.messages().is_empty());
			assert!(!state.is_streaming());
			assert_eq!(state.streaming_message(), None);
			assert!(state.pending_tool_calls().is_empty());
			assert_eq!(state.error_message(), None);
		});
	}

	#[test]
	fn should_create_an_agent_instance_with_custom_initial_state() {
		run(|_host| async {
			let custom_model = test_model("openai", "gpt-4o-mini", "openai-responses");
			let agent = agent_with(AgentOptions {
				stream_fn: Some(unused_stream_function()),
				initial_state: Some(AgentInitialState {
					system_prompt: Some("You are a helpful assistant.".into()),
					model: Some(custom_model.clone()),
					thinking_level: Some(ThinkingLevel::Low),
					..AgentInitialState::default()
				}),
				..AgentOptions::default()
			});

			assert_eq!(
				js_json(&agent.state().messages()),
				json!([{ "role": "system", "content": "You are a helpful assistant.", "timestamp": 0 }])
			);
			assert_eq!(agent.state().model, custom_model);
			assert_eq!(agent.state().thinking_level, ThinkingLevel::Low);
		});
	}

	#[test]
	fn converts_initial_prompt_and_tools_into_transcript_state() {
		run(|_host| async {
			let tool = tool("echo", "Echo", "Echo input", returning(text_result("echo")));
			let agent = agent_with(AgentOptions {
				initial_state: Some(AgentInitialState {
					system_prompt: Some("You are helpful.".into()),
					tools: Some(vec![tool]),
					..AgentInitialState::default()
				}),
				stream_fn: Some(unused_stream_function()),
				..AgentOptions::default()
			});

			let state = agent.state();
			let initial = system_message(&state.messages()[0]);
			assert_eq!(initial.content, SystemMessageContent::Text("You are helpful.".into()));
			assert_eq!(tool_names(&initial.tools_added), ["echo"]);
		});
	}

	#[test]
	fn declares_tool_loadout_changes_to_the_model_before_the_next_request() {
		run(|_host| async {
			let first = create_tool("first");
			let second = create_tool("second");
			let requests: Rc<RefCell<Vec<Vec<String>>>> = Rc::new(RefCell::new(Vec::new()));
			let recorded = requests.clone();
			let agent = agent_with(AgentOptions {
				initial_state: Some(AgentInitialState {
					system_prompt: Some("You are helpful.".into()),
					tools: Some(vec![first]),
					..AgentInitialState::default()
				}),
				stream_fn: Some(Rc::new(move |_model, context, _options| {
					recorded.borrow_mut().push(
						context
							.messages
							.iter()
							.flat_map(|message| match message {
								Message::System(message) => vec![
									format!("+{}", tool_names(&message.tools_added).join(",")),
									format!(
										"-{}",
										message
											.tools_removed
											.iter()
											.flatten()
											.map(|tool| tool.name.clone())
											.collect::<Vec<_>>()
											.join(",")
									),
								],
								_ => Vec::new(),
							})
							.collect(),
					);
					let stream = push_later(done(create_assistant_message("done")));
					Box::pin(async move { Ok(stream) })
				})),
				..AgentOptions::default()
			});

			agent.prompt("one").await.unwrap();
			agent.state_mut().set_tools(vec![second]);
			agent.prompt("two").await.unwrap();
			agent.prompt("three").await.unwrap();

			assert_eq!(
				*requests.borrow(),
				vec![
					vec!["+first", "-"],
					vec!["+first", "-", "+second", "-first"],
					vec!["+first", "-", "+second", "-first"],
				]
			);
			let state = agent.state();
			let update = state
				.messages()
				.iter()
				.find(
					|message| matches!(message, AgentMessage::Llm(Message::System(message)) if message.tools_removed.is_some()),
				)
				.expect("a tool update");
			let update = system_message(update);
			assert_eq!(update.content, SystemMessageContent::Text(String::new()));
			assert_eq!(
				update.tools_added,
				Some(vec![Tool {
					name: "second".into(),
					description: "second tool".into(),
					parameters: empty_obj(),
					..Tool::default()
				}])
			);
			assert_eq!(update.tools_removed, Some(vec![ToolReference { name: "first".into() }]));
			assert!(update.sections.is_none());
			assert!(update.extra.is_empty());
			let initial = system_message(&state.messages()[0]);
			let declared = serde_json::to_value(&initial.tools_added.as_ref().unwrap()[0]).unwrap();
			assert!(declared.get("execute").is_none());
		});
	}

	#[test]
	fn merges_tool_changes_into_a_pending_system_message() {
		run(|_host| async {
			let tool = tool("echo", "Echo", "Echo input", returning(text_result("echo")));
			let agent = agent_with(AgentOptions {
				initial_state: Some(AgentInitialState {
					system_prompt: Some("You are helpful.".into()),
					..AgentInitialState::default()
				}),
				stream_fn: Some(Rc::new(|_model, context, _options| {
					assert_eq!(
						context
							.messages
							.iter()
							.filter(|message| matches!(message, Message::System(_)))
							.count(),
						2
					);
					let stream = push_later(done(create_assistant_message("done")));
					Box::pin(async move { Ok(stream) })
				})),
				..AgentOptions::default()
			});

			agent.state_mut().set_tools(vec![tool]);
			let messages: Vec<AgentMessage> = serde_json::from_value(json!([
				{ "role": "system", "content": "", "sections": { "skills": "<skills>x</skills>" }, "timestamp": 1 },
				{ "role": "user", "content": "hi", "timestamp": 2 },
			]))
			.unwrap();
			agent.prompt(messages).await.unwrap();

			assert_eq!(
				js_json(&agent.state().messages()[1]),
				json!({
					"role": "system",
					"content": "",
					"sections": { "skills": "<skills>x</skills>" },
					"toolsAdded": [{ "name": "echo", "description": "Echo input", "parameters": empty_obj() }],
					"timestamp": 1,
				})
			);
		});
	}

	#[test]
	fn rewrites_pending_tool_declarations_to_match_the_executable_set() {
		run(|_host| async {
			let agent = agent_with(AgentOptions {
				initial_state: Some(AgentInitialState {
					system_prompt: Some("You are helpful.".into()),
					tools: Some(vec![create_tool("first")]),
					..AgentInitialState::default()
				}),
				stream_fn: Some(responding("done")),
				..AgentOptions::default()
			});

			// The pending message claims to add `second` and remove `first`, but the executable
			// set still has `first` and lacks `second`: the executable set wins.
			let pending = SystemMessage {
				content: SystemMessageContent::Text(String::new()),
				sections: Some(
					[("note".to_string(), Some("<note>x</note>".to_string()))]
						.into_iter()
						.collect(),
				),
				tools_added: Some(vec![to_tool_declaration(&create_tool("second").tool)]),
				tools_removed: Some(vec![ToolReference { name: "first".into() }]),
				timestamp: 1.0,
				..SystemMessage::default()
			};
			let user = UserMessage {
				content: UserMessageContent::Text("hi".into()),
				timestamp: 2.0,
				..UserMessage::default()
			};
			agent.prompt(vec![pending.into(), user.into()]).await.unwrap();

			assert_eq!(
				js_json(&agent.state().messages()[1]),
				json!({
					"role": "system",
					"content": "",
					"sections": { "note": "<note>x</note>" },
					"timestamp": 1,
				})
			);
			assert_eq!(
				tool_names(
					&get_current_system_message(agent.state().messages())
						.unwrap()
						.tools_added
				),
				["first"]
			);
		});
	}

	#[test]
	fn restores_the_transcript_baseline_when_reset() {
		run(|_host| async {
			let tool = tool("echo", "Echo", "Echo input", returning(text_result("echo")));
			let agent = agent_with(AgentOptions {
				initial_state: Some(AgentInitialState {
					system_prompt: Some("You are helpful.".into()),
					tools: Some(vec![tool]),
					messages: Some(vec![
						UserMessage {
							content: UserMessageContent::Text("old".into()),
							timestamp: 1.0,
							..UserMessage::default()
						}
						.into(),
					]),
					..AgentInitialState::default()
				}),
				stream_fn: Some(unused_stream_function()),
				..AgentOptions::default()
			});

			agent.reset().unwrap();

			let state = agent.state();
			assert_eq!(state.messages().len(), 1);
			let initial = system_message(&state.messages()[0]);
			assert_eq!(initial.content, SystemMessageContent::Text("You are helpful.".into()));
			assert_eq!(tool_names(&initial.tools_added), ["echo"]);
		});
	}

	#[test]
	fn should_subscribe_to_events() {
		run(|_host| async {
			let agent = agent_with(AgentOptions {
				stream_fn: Some(unused_stream_function()),
				..AgentOptions::default()
			});

			let event_count = Rc::new(Cell::new(0));
			let counter = event_count.clone();
			let unsubscribe = agent.subscribe(listener(move |_event, _signal| counter.set(counter.get() + 1)));

			// No initial event on subscribe
			assert_eq!(event_count.get(), 0);

			// State mutators don't emit events
			agent.state_mut().thinking_level = ThinkingLevel::Low;
			assert_eq!(event_count.get(), 0);
			assert_eq!(agent.state().thinking_level, ThinkingLevel::Low);

			// Unsubscribe should work
			unsubscribe();
			agent.state_mut().thinking_level = ThinkingLevel::High;
			assert_eq!(event_count.get(), 0); // Should not increase
		});
	}

	#[test]
	fn emits_full_lifecycle_events_for_thrown_run_failures() {
		run(|_host| async {
			let agent = agent_with(AgentOptions {
				stream_fn: Some(Rc::new(|_model, _context, _options| {
					Box::pin(async { Err(PiError::new("provider exploded")) })
				})),
				..AgentOptions::default()
			});
			let events: Rc<RefCell<Vec<String>>> = Rc::new(RefCell::new(Vec::new()));
			let recorded = events.clone();
			let _ = agent.subscribe(listener(move |event, _signal| {
				recorded.borrow_mut().push(event_type(event))
			}));

			agent.prompt("hello").await.unwrap();

			assert_eq!(
				*events.borrow(),
				[
					"agent_start",
					"turn_start",
					"message_start",
					"message_end",
					"message_start",
					"message_end",
					"turn_end",
					"agent_end",
				]
			);
			let state = agent.state();
			let Some(AgentMessage::Llm(Message::Assistant(last_message))) = state.messages().last() else {
				panic!("Expected assistant message");
			};
			assert_eq!(last_message.stop_reason, StopReason::Error);
			assert_eq!(last_message.error_message.as_deref(), Some("provider exploded"));
			assert_eq!(state.error_message(), Some("provider exploded"));
		});
	}

	#[test]
	fn should_await_async_subscribers_before_prompt_resolves() {
		run(|_host| async {
			let barrier = create_deferred();
			let agent = agent_with(AgentOptions {
				stream_fn: Some(responding("ok")),
				..AgentOptions::default()
			});

			let listener_finished = Rc::new(Cell::new(false));
			{
				let barrier = barrier.clone();
				let listener_finished = listener_finished.clone();
				let _ = agent.subscribe(Rc::new(move |event, _signal| {
					let barrier = barrier.clone();
					let listener_finished = listener_finished.clone();
					Box::pin(async move {
						if matches!(event, AgentEvent::AgentEnd { .. }) {
							barrier.promise.clone().await;
							listener_finished.set(true);
						}
						Ok(())
					})
				}));
			}

			let prompt_resolved = Rc::new(Cell::new(false));
			let (resolve, prompt_promise) = oneshot::channel();
			{
				let agent = agent.clone();
				let prompt_resolved = prompt_resolved.clone();
				pi_host::spawn_local(async move {
					agent.prompt("hello").await.unwrap();
					prompt_resolved.set(true);
					let _ = resolve.send(());
				});
			}

			pi_host::sleep(10.0, None).await.unwrap();
			assert!(!prompt_resolved.get());
			assert!(!listener_finished.get());
			assert!(agent.state().is_streaming());

			barrier.resolve();
			prompt_promise.await.unwrap();

			assert!(listener_finished.get());
			assert!(prompt_resolved.get());
			assert!(!agent.state().is_streaming());
		});
	}

	#[test]
	fn wait_for_idle_should_wait_for_async_subscribers() {
		run(|_host| async {
			let barrier = create_deferred();
			let agent = agent_with(AgentOptions {
				stream_fn: Some(responding("ok")),
				..AgentOptions::default()
			});

			{
				let barrier = barrier.clone();
				let _ = agent.subscribe(Rc::new(move |event, _signal| {
					let barrier = barrier.clone();
					Box::pin(async move {
						if let AgentEvent::MessageEnd { message } = &event
							&& message.role() == "assistant"
						{
							barrier.promise.clone().await;
						}
						Ok(())
					})
				}));
			}

			let prompt_promise = spawn_prompt(&agent, "hello");
			// The prompt starts on the next tick; `waitForIdle()` is called once the run is active.
			pi_host::sleep(0.0, None).await.unwrap();
			let idle_resolved = Rc::new(Cell::new(false));
			let idle_promise = {
				let idle_resolved = idle_resolved.clone();
				let idle = agent.wait_for_idle();
				async move {
					idle.await;
					idle_resolved.set(true);
				}
				.boxed_local()
				.shared()
			};
			pi_host::spawn_local(idle_promise.clone());

			pi_host::sleep(10.0, None).await.unwrap();
			assert!(!idle_resolved.get());
			assert!(agent.state().is_streaming());

			barrier.resolve();
			let (prompt_result, ()) = futures::join!(prompt_promise, idle_promise);
			prompt_result.unwrap().unwrap();

			assert!(idle_resolved.get());
			assert!(!agent.state().is_streaming());
		});
	}

	#[test]
	fn should_pass_the_active_abort_signal_to_subscribers() {
		run(|_host| async {
			let received_signal: Rc<RefCell<Option<AbortSignal>>> = Rc::new(RefCell::new(None));
			let agent = agent_with(AgentOptions {
				stream_fn: Some(Rc::new(|_model, _context, options| {
					let stream = abortable_stream(options.and_then(|options| options.signal.clone()));
					Box::pin(async move { Ok(stream) })
				})),
				..AgentOptions::default()
			});

			{
				let received_signal = received_signal.clone();
				let _ = agent.subscribe(listener(move |event, signal| {
					if matches!(event, AgentEvent::AgentStart) {
						*received_signal.borrow_mut() = Some(signal.clone());
					}
				}));
			}

			let prompt_promise = spawn_prompt(&agent, "hello");
			pi_host::sleep(10.0, None).await.unwrap();

			assert!(received_signal.borrow().is_some());
			assert!(!received_signal.borrow().as_ref().unwrap().aborted());

			agent.abort();
			prompt_promise.await.unwrap().unwrap();

			assert!(received_signal.borrow().as_ref().unwrap().aborted());
		});
	}

	#[test]
	fn should_ignore_tool_updates_after_the_tool_execution_settles() {
		run(|_host| async {
			let delayed_update: Rc<RefCell<Option<AgentToolUpdateCallback>>> = Rc::new(RefCell::new(None));
			let events: Rc<RefCell<Vec<AgentEvent>>> = Rc::new(RefCell::new(Vec::new()));
			let execute: pi_agent::AgentToolExecute = {
				let delayed_update = delayed_update.clone();
				Rc::new(move |_id, _params, _signal, on_update| {
					*delayed_update.borrow_mut() = on_update.clone();
					if let Some(on_update) = &on_update {
						on_update(AgentToolResult {
							details: Some(json!({ "status": "running" })),
							..text_result("running")
						});
					}
					Box::pin(async {
						Ok(AgentToolResult {
							details: Some(json!({ "status": "done" })),
							terminate: Some(true),
							..text_result("ok")
						})
					})
				})
			};
			let tool = tool("delayed_tool", "Delayed Tool", "Captures progress callbacks", execute);
			let agent = agent_with(AgentOptions {
				initial_state: Some(AgentInitialState {
					tools: Some(vec![tool]),
					..AgentInitialState::default()
				}),
				stream_fn: Some(Rc::new(|_model, _context, _options| {
					let stream = push_later(done(create_assistant_tool_use_message(vec![tool_call(
						"call-1",
						"delayed_tool",
					)])));
					Box::pin(async move { Ok(stream) })
				})),
				..AgentOptions::default()
			});
			{
				let events = events.clone();
				let _ = agent.subscribe(listener(move |event, _signal| events.borrow_mut().push(event.clone())));
			}

			agent.prompt("run tool").await.unwrap();
			let event_count_after_prompt = events.borrow().len();

			let late = delayed_update.borrow().clone();
			if let Some(late) = late {
				late(AgentToolResult {
					details: Some(json!({ "status": "late" })),
					..text_result("late")
				});
			}
			pi_host::sleep(0.0, None).await.unwrap();

			assert_eq!(
				events
					.borrow()
					.iter()
					.filter(|event| matches!(event, AgentEvent::ToolExecutionUpdate { .. }))
					.count(),
				1
			);
			assert_eq!(events.borrow().len(), event_count_after_prompt);
		});
	}

	#[test]
	fn should_ignore_a_settled_parallel_tool_update_while_another_tool_is_still_running() {
		run(|_host| async {
			let slow_started = create_deferred();
			let settled_tool_ended = create_deferred();
			let release_slow = create_deferred();
			let settled_tool_update: Rc<RefCell<Option<AgentToolUpdateCallback>>> = Rc::new(RefCell::new(None));
			let events: Rc<RefCell<Vec<AgentEvent>>> = Rc::new(RefCell::new(Vec::new()));
			let settled_tool = tool("settled_tool", "Settled Tool", "Captures progress callbacks", {
				let settled_tool_update = settled_tool_update.clone();
				Rc::new(move |_id, _params, _signal, on_update| {
					*settled_tool_update.borrow_mut() = on_update;
					Box::pin(async {
						Ok(AgentToolResult {
							details: Some(json!({ "status": "done" })),
							terminate: Some(true),
							..text_result("done")
						})
					})
				})
			});
			let slow_tool = tool("slow_tool", "Slow Tool", "Keeps the agent run active", {
				let slow_started = slow_started.clone();
				let release_slow = release_slow.clone();
				Rc::new(move |_id, _params, _signal, _on_update| {
					let slow_started = slow_started.clone();
					let release_slow = release_slow.clone();
					Box::pin(async move {
						slow_started.resolve();
						release_slow.promise.clone().await;
						Ok(AgentToolResult {
							details: Some(json!({ "status": "done" })),
							terminate: Some(true),
							..text_result("done")
						})
					})
				})
			});
			let agent = agent_with(AgentOptions {
				initial_state: Some(AgentInitialState {
					tools: Some(vec![settled_tool, slow_tool]),
					..AgentInitialState::default()
				}),
				stream_fn: Some(Rc::new(|_model, _context, _options| {
					let stream = push_later(done(create_assistant_tool_use_message(vec![
						tool_call("call-1", "settled_tool"),
						tool_call("call-2", "slow_tool"),
					])));
					Box::pin(async move { Ok(stream) })
				})),
				..AgentOptions::default()
			});
			{
				let events = events.clone();
				let settled_tool_ended = settled_tool_ended.clone();
				let _ = agent.subscribe(listener(move |event, _signal| {
					events.borrow_mut().push(event.clone());
					if let AgentEvent::ToolExecutionEnd { tool_call_id, .. } = event
						&& tool_call_id == "call-1"
					{
						settled_tool_ended.resolve();
					}
				}));
			}

			let prompt_promise = spawn_prompt(&agent, "run tools");
			futures::join!(slow_started.promise.clone(), settled_tool_ended.promise.clone());
			let event_count_before_late_update = events.borrow().len();

			let late = settled_tool_update.borrow().clone();
			if let Some(late) = late {
				late(AgentToolResult {
					details: Some(json!({ "status": "late" })),
					..text_result("late")
				});
			}
			pi_host::sleep(0.0, None).await.unwrap();
			assert_eq!(events.borrow().len(), event_count_before_late_update);

			release_slow.resolve();
			prompt_promise.await.unwrap().unwrap();
			assert_eq!(
				events
					.borrow()
					.iter()
					.filter(|event| matches!(event, AgentEvent::ToolExecutionUpdate { .. }))
					.count(),
				0
			);
		});
	}

	#[test]
	fn should_update_state_with_mutators() {
		run(|_host| async {
			let agent = agent_with(AgentOptions {
				stream_fn: Some(unused_stream_function()),
				..AgentOptions::default()
			});

			// Test setModel
			let new_model = test_model("google", "gemini-2.5-flash", "google-generative-ai");
			agent.state_mut().model = new_model.clone();
			assert_eq!(agent.state().model, new_model);

			// Test setThinkingLevel
			agent.state_mut().thinking_level = ThinkingLevel::High;
			assert_eq!(agent.state().thinking_level, ThinkingLevel::High);

			// Test setTools
			let tools = vec![AgentTool {
				tool: Tool {
					name: "test".into(),
					description: "test tool".into(),
					..Tool::default()
				},
				..create_tool("test")
			}];
			agent.state_mut().set_tools(tools.clone());
			let state_tools: Vec<Tool> = agent.state().tools().iter().map(|tool| tool.tool.clone()).collect();
			assert_eq!(
				state_tools,
				tools.iter().map(|tool| tool.tool.clone()).collect::<Vec<_>>()
			);

			// Test replaceMessages
			let messages: Vec<AgentMessage> = vec![
				UserMessage {
					content: UserMessageContent::Text("Hello".into()),
					timestamp: pi_host::now_ms(),
					..UserMessage::default()
				}
				.into(),
			];
			agent.state_mut().set_messages(messages.clone());
			assert_eq!(agent.state().messages(), messages.as_slice());

			// Test appendMessage
			let new_message: AgentMessage = create_assistant_message("Hi").into();
			agent.state_mut().messages_mut().push(new_message.clone());
			assert_eq!(agent.state().messages().len(), 2);
			assert_eq!(agent.state().messages()[1], new_message);

			// Test clearMessages
			agent.state_mut().set_messages(Vec::new());
			assert!(agent.state().messages().is_empty());
		});
	}

	#[test]
	fn should_support_steering_message_queue() {
		run(|_host| async {
			let agent = agent_with(AgentOptions {
				stream_fn: Some(unused_stream_function()),
				..AgentOptions::default()
			});

			let message: AgentMessage = UserMessage {
				content: UserMessageContent::Text("Steering message".into()),
				timestamp: pi_host::now_ms(),
				..UserMessage::default()
			}
			.into();
			agent.steer(message.clone());

			// The message is queued but not yet in state.messages
			assert!(!agent.state().messages().contains(&message));
		});
	}

	#[test]
	fn should_support_follow_up_message_queue() {
		run(|_host| async {
			let agent = agent_with(AgentOptions {
				stream_fn: Some(unused_stream_function()),
				..AgentOptions::default()
			});

			let message: AgentMessage = UserMessage {
				content: UserMessageContent::Text("Follow-up message".into()),
				timestamp: pi_host::now_ms(),
				..UserMessage::default()
			}
			.into();
			agent.follow_up(message.clone());

			// The message is queued but not yet in state.messages
			assert!(!agent.state().messages().contains(&message));
		});
	}

	#[test]
	fn should_handle_abort_controller() {
		run(|_host| async {
			let agent = agent_with(AgentOptions {
				stream_fn: Some(unused_stream_function()),
				..AgentOptions::default()
			});

			// Should not throw even if nothing is running
			agent.abort();
		});
	}

	#[test]
	fn should_reject_reset_while_processing_without_corrupting_the_transcript() {
		run(|_host| async {
			let stream_started = create_deferred();
			let release_response = create_deferred();
			let agent = agent_with(AgentOptions {
				stream_fn: Some({
					let stream_started = stream_started.clone();
					let release_response = release_response.clone();
					Rc::new(move |_model, _context, _options| {
						let stream = AssistantMessageEventStream::new();
						let output = stream.clone();
						let stream_started = stream_started.clone();
						let release_response = release_response.clone();
						pi_host::spawn_local(async move {
							output.push(AssistantMessageEvent::Start {
								partial: Rc::new(RefCell::new(create_assistant_message(""))),
							});
							stream_started.resolve();
							release_response.promise.clone().await;
							output.push(done(create_assistant_message("Done")));
						});
						Box::pin(async move { Ok(stream) })
					})
				}),
				..AgentOptions::default()
			});

			let prompt_promise = spawn_prompt(&agent, "Hello");
			stream_started.promise.clone().await;

			let roles = |agent: &Agent| -> Vec<String> {
				agent
					.state()
					.messages()
					.iter()
					.map(|message| message.role().to_string())
					.collect()
			};
			assert!(agent.state().is_streaming());
			assert_eq!(roles(&agent), ["user"]);
			assert_eq!(
				agent.reset().unwrap_err().message,
				"Agent is already processing. Wait for completion before resetting."
			);
			assert!(agent.state().is_streaming());
			assert_eq!(roles(&agent), ["user"]);
			release_response.resolve();
			prompt_promise.await.unwrap().unwrap();

			assert!(!agent.state().is_streaming());
			assert_eq!(roles(&agent), ["user", "assistant"]);
		});
	}

	#[test]
	fn should_throw_when_prompt_called_while_streaming() {
		run(|_host| async {
			let agent = agent_with(AgentOptions {
				// Use a stream function that responds to abort
				stream_fn: Some(Rc::new(|_model, _context, options| {
					let stream = abortable_stream(options.and_then(|options| options.signal.clone()));
					Box::pin(async move { Ok(stream) })
				})),
				..AgentOptions::default()
			});

			// Start first prompt (don't await, it will block until abort)
			let first_prompt = spawn_prompt(&agent, "First message");

			// Wait a tick for isStreaming to be set
			pi_host::sleep(10.0, None).await.unwrap();
			assert!(agent.state().is_streaming());

			// Second prompt should reject
			assert_eq!(
				agent.prompt("Second message").await.unwrap_err().message,
				"Agent is already processing a prompt. Use steer() or followUp() to queue messages, or wait for completion."
			);

			// Cleanup - abort to stop the stream
			agent.abort();
			let _ = first_prompt.await; // Ignore abort error
		});
	}

	#[test]
	fn should_throw_when_continue_called_while_streaming() {
		run(|_host| async {
			let agent = agent_with(AgentOptions {
				stream_fn: Some(Rc::new(|_model, _context, options| {
					let stream = abortable_stream(options.and_then(|options| options.signal.clone()));
					Box::pin(async move { Ok(stream) })
				})),
				..AgentOptions::default()
			});

			// Start first prompt
			let first_prompt = spawn_prompt(&agent, "First message");
			pi_host::sleep(10.0, None).await.unwrap();
			assert!(agent.state().is_streaming());

			// continue() should reject
			assert_eq!(
				agent.r#continue().await.unwrap_err().message,
				"Agent is already processing. Wait for completion before continuing."
			);

			// Cleanup
			agent.abort();
			let _ = first_prompt.await;
		});
	}

	#[test]
	fn continue_should_process_queued_follow_up_messages_after_an_assistant_turn() {
		run(|_host| async {
			let agent = agent_with(AgentOptions {
				stream_fn: Some(responding("Processed")),
				..AgentOptions::default()
			});

			agent.state_mut().set_messages(vec![
				UserMessage {
					content: UserMessageContent::Blocks(vec![UserContent::Text(TextContent {
						text: "Initial".into(),
						..TextContent::default()
					})]),
					timestamp: pi_host::now_ms() - 10.0,
					..UserMessage::default()
				}
				.into(),
				create_assistant_message("Initial response").into(),
			]);

			agent.follow_up(
				UserMessage {
					content: UserMessageContent::Blocks(vec![UserContent::Text(TextContent {
						text: "Queued follow-up".into(),
						..TextContent::default()
					})]),
					timestamp: pi_host::now_ms(),
					..UserMessage::default()
				}
				.into(),
			);

			agent.r#continue().await.unwrap();

			let state = agent.state();
			let has_queued_follow_up = state.messages().iter().any(|message| match message {
				AgentMessage::Llm(Message::User(UserMessage { content, .. })) => match content {
					UserMessageContent::Text(text) => text == "Queued follow-up",
					UserMessageContent::Blocks(blocks) => blocks
						.iter()
						.any(|part| matches!(part, UserContent::Text(text) if text.text == "Queued follow-up")),
				},
				_ => false,
			});

			assert!(has_queued_follow_up);
			assert_eq!(state.messages().last().unwrap().role(), "assistant");
		});
	}

	async fn continue_keeps_steering_semantics_for_assistant_tail_fallback(mode: QueueMode, expected_requests: usize) {
		let requests: Rc<RefCell<Vec<Vec<String>>>> = Rc::new(RefCell::new(Vec::new()));
		let agent = agent_with(AgentOptions {
			steering_mode: Some(mode),
			stream_fn: Some({
				let requests = requests.clone();
				Rc::new(move |_model, context, _options| {
					requests.borrow_mut().push(user_texts(context));
					let stream = push_later(done(create_assistant_message("Processed")));
					Box::pin(async move { Ok(stream) })
				})
			}),
			..AgentOptions::default()
		});
		agent.state_mut().set_messages(vec![
			create_user_message("Initial").into(),
			create_assistant_message("Initial response").into(),
		]);
		agent.steer(create_user_message("Steering 1").into());
		agent.steer(create_user_message("Steering 2").into());

		agent.r#continue().await.unwrap();

		let requests = requests.borrow();
		assert_eq!(requests.len(), expected_requests);
		assert!(requests[0].contains(&"Steering 1".to_string()));
		if mode == QueueMode::OneAtATime {
			assert!(!requests[0].contains(&"Steering 2".to_string()));
			assert!(requests[1].contains(&"Steering 2".to_string()));
		} else {
			assert!(requests[0].contains(&"Steering 2".to_string()));
		}
	}

	#[test]
	fn continue_keeps_one_at_a_time_steering_semantics_for_assistant_tail_fallback() {
		run(|_host| continue_keeps_steering_semantics_for_assistant_tail_fallback(QueueMode::OneAtATime, 2));
	}

	#[test]
	fn continue_keeps_all_steering_semantics_for_assistant_tail_fallback() {
		run(|_host| continue_keeps_steering_semantics_for_assistant_tail_fallback(QueueMode::All, 1));
	}

	#[test]
	fn keeps_legacy_prepare_next_turn_signal_callback_behavior() {
		run(|_host| async {
			let tool = tool("noop", "Noop", "Noop tool", returning(text_result("ok")));
			let request_count = Rc::new(Cell::new(0));
			let saw_abort_signal = Rc::new(Cell::new(false));
			let agent = agent_with(AgentOptions {
				initial_state: Some(AgentInitialState {
					tools: Some(vec![tool]),
					..AgentInitialState::default()
				}),
				prepare_next_turn: Some({
					let saw_abort_signal = saw_abort_signal.clone();
					Rc::new(move |signal| {
						saw_abort_signal.set(signal.is_some());
						Box::pin(async { Ok(None) })
					})
				}),
				stream_fn: Some({
					let request_count = request_count.clone();
					Rc::new(move |_model, _context, _options| {
						request_count.set(request_count.get() + 1);
						let message = if request_count.get() == 1 {
							create_assistant_tool_use_message(vec![tool_call("tool-1", "noop")])
						} else {
							create_assistant_message("done")
						};
						let stream = push_later(done(message));
						Box::pin(async move { Ok(stream) })
					})
				}),
				..AgentOptions::default()
			});

			agent.prompt("start").await.unwrap();

			assert_eq!(request_count.get(), 2);
			assert!(saw_abort_signal.get());
		});
	}

	#[test]
	fn forwards_finish_turn_through_agent_options_with_the_active_abort_signal() {
		run(|_host| async {
			let tool = tool("noop", "Noop", "Noop tool", returning(text_result("tool complete")));
			let request_count = Rc::new(Cell::new(0));
			let saw_abort_signal = Rc::new(Cell::new(false));
			let callback_context_roles: Rc<RefCell<Vec<String>>> = Rc::new(RefCell::new(Vec::new()));
			let agent = agent_with(AgentOptions {
				initial_state: Some(AgentInitialState {
					tools: Some(vec![tool]),
					..AgentInitialState::default()
				}),
				finish_turn: Some({
					let saw_abort_signal = saw_abort_signal.clone();
					let callback_context_roles = callback_context_roles.clone();
					Rc::new(move |context, signal| {
						saw_abort_signal.set(signal.is_some());
						*callback_context_roles.borrow_mut() = context
							.context
							.messages
							.iter()
							.map(|message| message.role().to_string())
							.collect();
						Box::pin(async { Ok(Some(AgentTurnDecision::End)) })
					})
				}),
				stream_fn: Some({
					let request_count = request_count.clone();
					Rc::new(move |_model, _context, _options| {
						request_count.set(request_count.get() + 1);
						let message = if request_count.get() == 1 {
							create_assistant_tool_use_message(vec![tool_call("tool-1", "noop")])
						} else {
							create_assistant_message("should not run")
						};
						let stream = push_later(done(message));
						Box::pin(async move { Ok(stream) })
					})
				}),
				..AgentOptions::default()
			});

			agent.prompt("start").await.unwrap();

			assert_eq!(request_count.get(), 1);
			assert!(saw_abort_signal.get());
			assert_eq!(
				*callback_context_roles.borrow(),
				["system", "user", "assistant", "toolResult"]
			);
		});
	}

	async fn rejects_a_queued_continuation_without_draining_queues(messages: Vec<AgentMessage>) {
		let agent = agent_with(AgentOptions {
			initial_state: Some(AgentInitialState {
				messages: Some(messages),
				..AgentInitialState::default()
			}),
			stream_fn: Some(unused_stream_function()),
			..AgentOptions::default()
		});
		let steering: AgentMessage = create_user_message("steering").into();
		let follow_up: AgentMessage = create_user_message("follow-up").into();
		agent.steer(steering.clone());
		agent.follow_up(follow_up.clone());

		assert_eq!(
			agent.r#continue().await.unwrap_err().message,
			"No messages to continue from"
		);
		assert_eq!(agent.peek_queued_messages(), [steering]);
		agent.clear_steering_queue();
		assert_eq!(agent.peek_queued_messages(), [follow_up]);
	}

	#[test]
	fn rejects_a_queued_continuation_from_empty_context_without_draining_queues() {
		run(|_host| rejects_a_queued_continuation_without_draining_queues(Vec::new()));
	}

	#[test]
	fn rejects_a_queued_continuation_from_system_only_context_without_draining_queues() {
		run(|_host| {
			rejects_a_queued_continuation_without_draining_queues(vec![
				SystemMessage {
					content: SystemMessageContent::Text("system only".into()),
					timestamp: 1.0,
					..SystemMessage::default()
				}
				.into(),
			])
		});
	}

	async fn defers_follow_up_input_on_the_first_continuation_request(messages: Vec<AgentMessage>) {
		let requests: Rc<RefCell<Vec<Vec<String>>>> = Rc::new(RefCell::new(Vec::new()));
		let agent = agent_with(AgentOptions {
			initial_state: Some(AgentInitialState {
				messages: Some(messages),
				..AgentInitialState::default()
			}),
			stream_fn: Some(recording_user_texts(&requests)),
			..AgentOptions::default()
		});
		agent.follow_up(create_user_message("follow-up").into());

		agent.r#continue().await.unwrap();

		let requests = requests.borrow();
		assert_eq!(requests.len(), 2);
		assert!(!requests[0].contains(&"follow-up".to_string()));
		assert!(requests[1].contains(&"follow-up".to_string()));
	}

	#[test]
	fn defers_follow_up_input_on_the_first_continuation_request_from_a_user_tail() {
		run(|_host| {
			defers_follow_up_input_on_the_first_continuation_request(vec![create_user_message("existing user").into()])
		});
	}

	#[test]
	fn defers_follow_up_input_on_the_first_continuation_request_from_a_tool_result_tail() {
		run(|_host| {
			defers_follow_up_input_on_the_first_continuation_request(vec![
				create_user_message("existing user").into(),
				create_assistant_tool_use_message(vec![tool_call("call-1", "noop")]).into(),
				ToolResultMessage {
					tool_call_id: "call-1".into(),
					tool_name: "noop".into(),
					content: vec![UserContent::Text(TextContent {
						text: "done".into(),
						..TextContent::default()
					})],
					is_error: false,
					timestamp: 1.0,
					..ToolResultMessage::default()
				}
				.into(),
			])
		});
	}

	async fn polls_steering_at_continuation_startup(mode: QueueMode, expected_requests: usize) {
		let requests: Rc<RefCell<Vec<Vec<String>>>> = Rc::new(RefCell::new(Vec::new()));
		let agent = agent_with(AgentOptions {
			initial_state: Some(AgentInitialState {
				messages: Some(vec![create_user_message("existing").into()]),
				..AgentInitialState::default()
			}),
			steering_mode: Some(mode),
			stream_fn: Some(recording_user_texts(&requests)),
			..AgentOptions::default()
		});
		agent.steer(create_user_message("first").into());
		agent.steer(create_user_message("second").into());

		agent.r#continue().await.unwrap();

		let requests = requests.borrow();
		assert_eq!(requests.len(), expected_requests);
		assert!(requests[0].contains(&"first".to_string()));
		if mode == QueueMode::OneAtATime {
			assert!(!requests[0].contains(&"second".to_string()));
			assert!(requests[1].contains(&"second".to_string()));
		} else {
			assert!(requests[0].contains(&"second".to_string()));
		}
	}

	#[test]
	fn polls_one_at_a_time_steering_at_continuation_startup() {
		run(|_host| polls_steering_at_continuation_startup(QueueMode::OneAtATime, 2));
	}

	#[test]
	fn polls_all_steering_at_continuation_startup() {
		run(|_host| polls_steering_at_continuation_startup(QueueMode::All, 1));
	}

	#[test]
	fn keeps_steering_ahead_of_follow_up_from_a_non_assistant_continuation_tail() {
		run(|_host| async {
			let requests: Rc<RefCell<Vec<Vec<String>>>> = Rc::new(RefCell::new(Vec::new()));
			let agent = agent_with(AgentOptions {
				initial_state: Some(AgentInitialState {
					messages: Some(vec![create_user_message("existing").into()]),
					..AgentInitialState::default()
				}),
				stream_fn: Some(recording_user_texts(&requests)),
				..AgentOptions::default()
			});
			agent.steer(create_user_message("steering").into());
			agent.follow_up(create_user_message("follow-up").into());

			agent.r#continue().await.unwrap();

			let requests = requests.borrow();
			assert_eq!(requests.len(), 2);
			assert!(requests[0].contains(&"steering".to_string()));
			assert!(!requests[0].contains(&"follow-up".to_string()));
			assert!(requests[1].contains(&"follow-up".to_string()));
		});
	}

	async fn keeps_queues_on_a_failed_response_even_when_finish_turn_requests_continuation(stop_reason: StopReason) {
		let label = serde_json::to_value(stop_reason).unwrap().as_str().unwrap().to_string();
		let queued_during_response: AgentMessage = create_user_message("steering").into();
		let follow_up: AgentMessage = create_user_message("follow-up").into();
		let agent = agent_with(AgentOptions {
			finish_turn: Some(Rc::new(|_context, _signal| {
				Box::pin(async { Ok(Some(AgentTurnDecision::Continue)) })
			})),
			stream_fn: Some(Rc::new(move |_model, _context, _options| {
				let error = AssistantMessage {
					stop_reason,
					error_message: Some(label.clone()),
					..create_assistant_message(&label)
				};
				let stream = push_later(AssistantMessageEvent::Error {
					reason: stop_reason,
					error: Rc::new(RefCell::new(error)),
				});
				Box::pin(async move { Ok(stream) })
			})),
			..AgentOptions::default()
		});
		agent.follow_up(follow_up.clone());
		{
			let steering_agent = agent.clone();
			let queued_during_response = queued_during_response.clone();
			let _ = agent.subscribe(listener(move |event, _signal| {
				if let AgentEvent::MessageEnd { message } = event
					&& message.role() == "assistant"
				{
					steering_agent.steer(queued_during_response.clone());
				}
			}));
		}

		agent.prompt("start").await.unwrap();

		assert_eq!(agent.peek_queued_messages(), [queued_during_response]);
		agent.clear_steering_queue();
		assert_eq!(agent.peek_queued_messages(), [follow_up]);
	}

	#[test]
	fn keeps_queues_on_a_error_response_even_when_finish_turn_requests_continuation() {
		run(|_host| keeps_queues_on_a_failed_response_even_when_finish_turn_requests_continuation(StopReason::Error));
	}

	#[test]
	fn keeps_queues_on_a_aborted_response_even_when_finish_turn_requests_continuation() {
		run(|_host| keeps_queues_on_a_failed_response_even_when_finish_turn_requests_continuation(StopReason::Aborted));
	}

	#[test]
	fn keeps_queues_when_finish_turn_ends_the_run() {
		run(|_host| async {
			let queued_during_response: AgentMessage = create_user_message("steering").into();
			let follow_up: AgentMessage = create_user_message("follow-up").into();
			let agent = agent_with(AgentOptions {
				finish_turn: Some(Rc::new(|_context, _signal| {
					Box::pin(async { Ok(Some(AgentTurnDecision::End)) })
				})),
				stream_fn: Some(responding("done")),
				..AgentOptions::default()
			});
			agent.follow_up(follow_up.clone());
			{
				let steering_agent = agent.clone();
				let queued_during_response = queued_during_response.clone();
				let _ = agent.subscribe(listener(move |event, _signal| {
					if let AgentEvent::MessageEnd { message } = event
						&& message.role() == "assistant"
					{
						steering_agent.steer(queued_during_response.clone());
					}
				}));
			}

			agent.prompt("start").await.unwrap();

			assert_eq!(agent.peek_queued_messages(), [queued_during_response]);
			agent.clear_steering_queue();
			assert_eq!(agent.peek_queued_messages(), [follow_up]);
		});
	}

	#[test]
	fn previews_the_next_selected_queued_messages_without_consuming_them() {
		run(|_host| async {
			let agent = agent_with(AgentOptions {
				steering_mode: Some(QueueMode::OneAtATime),
				follow_up_mode: Some(QueueMode::All),
				stream_fn: Some(Rc::new(|_model, _context, _options| {
					Box::pin(async { Ok(AssistantMessageEventStream::new()) })
				})),
				..AgentOptions::default()
			});
			let first: AgentMessage = create_user_message("first steering").into();
			let second: AgentMessage = create_user_message("second steering").into();
			let follow_up: AgentMessage = create_user_message("follow-up").into();
			agent.steer(first.clone());
			agent.steer(second);
			agent.follow_up(follow_up.clone());

			assert_eq!(agent.peek_queued_messages(), std::slice::from_ref(&first));
			assert_eq!(agent.peek_queued_messages(), [first]);
			agent.clear_steering_queue();
			assert_eq!(agent.peek_queued_messages(), [follow_up]);
		});
	}

	#[test]
	fn forwards_provider_stream_event_observers_through_agent_options() {
		run(|_host| async {
			let provider_events: Rc<RefCell<Vec<Value>>> = Rc::new(RefCell::new(Vec::new()));
			let agent = agent_with(AgentOptions {
				on_provider_stream_event: Some({
					let provider_events = provider_events.clone();
					Rc::new(move |data, _model| {
						provider_events.borrow_mut().push(data.clone());
						Box::pin(async { Ok(()) })
					})
				}),
				stream_fn: Some(Rc::new(|model, _context, options| {
					let stream = AssistantMessageEventStream::new();
					let output = stream.clone();
					let model = model.clone();
					let observer = options.and_then(|options| options.on_provider_stream_event.clone());
					pi_host::spawn_local(async move {
						if let Some(observer) = observer {
							observer(&json!({ "request_cost": 0.01 }), &model).await.unwrap();
						}
						output.push(done(create_assistant_message("ok")));
					});
					Box::pin(async move { Ok(stream) })
				})),
				..AgentOptions::default()
			});

			agent.prompt("hello").await.unwrap();

			assert_eq!(*provider_events.borrow(), [json!({ "request_cost": 0.01 })]);
		});
	}

	#[test]
	fn forwards_session_id_to_stream_function_options() {
		run(|_host| async {
			let received_session_id: Rc<RefCell<Option<String>>> = Rc::new(RefCell::new(None));
			let agent = agent_with(AgentOptions {
				session_id: Some("session-abc".into()),
				stream_fn: Some({
					let received_session_id = received_session_id.clone();
					Rc::new(move |_model, _context, options| {
						*received_session_id.borrow_mut() = options.and_then(|options| options.session_id.clone());
						let stream = push_later(done(create_assistant_message("ok")));
						Box::pin(async move { Ok(stream) })
					})
				}),
				..AgentOptions::default()
			});

			agent.prompt("hello").await.unwrap();
			assert_eq!(received_session_id.borrow().as_deref(), Some("session-abc"));

			// Test setter
			agent.properties_mut().session_id = Some("session-def".into());
			assert_eq!(agent.properties().session_id.as_deref(), Some("session-def"));

			agent.prompt("hello again").await.unwrap();
			assert_eq!(received_session_id.borrow().as_deref(), Some("session-def"));
		});
	}

	#[test]
	fn attributes_requests_to_pi_by_default_and_merges_a_configured_identity_over_it() {
		run(|_host| async {
			let received: Rc<RefCell<Vec<Option<ClientIdentity>>>> = Rc::new(RefCell::new(Vec::new()));
			let agent = agent_with(AgentOptions {
				stream_fn: Some({
					let received = received.clone();
					Rc::new(move |_model, _context, options| {
						received
							.borrow_mut()
							.push(options.and_then(|options| options.identity.clone()));
						let stream = push_later(done(create_assistant_message("ok")));
						Box::pin(async move { Ok(stream) })
					})
				}),
				..AgentOptions::default()
			});

			agent.prompt("hello").await.unwrap();
			let acme = ClientAttribution {
				name: "acme".into(),
				url: "https://acme.example".into(),
				categories: None,
			};
			let short = UserAgentMode::Named(UserAgentModeName::Short);
			agent.properties_mut().identity = Some(ClientIdentity {
				user_agent: Some(short.clone()),
				attribution: Some(Some(acme.clone())),
				version: None,
			});
			agent.prompt("hello again").await.unwrap();
			agent.properties_mut().identity = Some(ClientIdentity {
				user_agent: Some(short.clone()),
				..ClientIdentity::default()
			});
			agent.prompt("and again").await.unwrap();

			assert_eq!(
				*received.borrow(),
				[
					Some(ClientIdentity {
						attribution: Some(Some(PI_ATTRIBUTION.clone())),
						..ClientIdentity::default()
					}),
					Some(ClientIdentity {
						attribution: Some(Some(acme)),
						user_agent: Some(short.clone()),
						version: None,
					}),
					Some(ClientIdentity {
						attribution: Some(Some(PI_ATTRIBUTION.clone())),
						user_agent: Some(short),
						version: None,
					}),
				]
			);
		});
	}
}
