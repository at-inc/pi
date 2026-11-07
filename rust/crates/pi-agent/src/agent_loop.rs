//! Port of `packages/agent/src/agent-loop.ts`.
//! PORT-STATUS: reviewed
//!
//! Agent loop that works with AgentMessage throughout.
//! Transforms to Message[] only at the LLM call boundary.
//!
//! Rust differences:
//! - The loop owns its context. TS `runAgentLoopContinue` shares the caller's `context.messages` array and pushes
//!   into it; the Rust loop takes the context by value and leaves the caller's transcript unchanged. Hooks receive
//!   snapshots (see `types.rs`).
//! - The streamed partial is not stored in the loop's context while the response streams; the final message is
//!   appended when the response ends, after the partials of any earlier `start` events (read when the response
//!   ends, like the live TS objects). No hook runs in between, so the context the hooks see matches TS.
//! - `agentLoop`/`agentLoopContinue` run the loop as a spawned task: nothing runs (no `agent_start`, no system
//!   message timestamp) until the task is first polled, where TS runs up to the first `await` before returning. A
//!   run that fails is dropped and its stream never ends; TS leaves an unhandled rejection.
//! - The stream function receives `config.base` (the stream options) plus `apiKey`, `identity` and `signal`. TS
//!   spreads the whole config, so a custom stream function there also sees `model`, `toolExecution` and the hooks.
//!   Extra provider keys travel in `config.base.extra`.
//! - `transformContext` receives a copy of the messages, so in-place edits of the array are lost; use its result.
//!   A tool's `execute` receives its arguments by value, so `afterToolCall` sees the arguments as prepared, not as
//!   the tool may have mutated them.
//! - When several `onUpdate` emits of one tool fail, the error of the first update is returned, not the first to
//!   fail in time.
//! - The loop's messages serialize in pi-ai's struct field order (`ToolResultMessage` puts `timestamp` before
//!   `durationMs`, `AssistantMessage.thinkingLevel` sits before `usage`, `SystemMessage` tool changes before
//!   `timestamp`). TS appends those keys last. Code that sends a transcript as JSON must order the keys itself.
//! - A tool's `onUpdate` starts its `tool_execution_update` emit as a spawned task that runs when the tool next
//!   yields. In TS the synchronous part of the emit (the agent's state update and the first listener call) runs
//!   inside `onUpdate`.
//! - When an emit fails during parallel tool execution, the other tools of the batch are dropped. TS rejects the
//!   `Promise.all` and leaves them running detached.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::LazyLock;

use futures::channel::oneshot;
use futures::stream::{FuturesUnordered, StreamExt};
use pi_ai::types::{
	AssistantContent, AssistantMessage, AssistantMessageEvent, Context, ModelThinkingLevel, SharedAssistantMessage,
	StopReason, SystemMessage, SystemMessageContent, TextContent, ThinkingLevel as ReasoningLevel, ToolResultMessage,
	UserContent,
};
use pi_ai::utils::event_stream::{AssistantMessageEventStream, EventStream};
use pi_ai::utils::identity::{ClientIdentity, PI_ATTRIBUTION, merge_client_identity};
use pi_ai::utils::transcript::{
	ToolStateChanges, TranscriptMessage, get_current_tools, get_tool_state_changes, normalize_context,
	to_tool_declaration,
};
use pi_ai::utils::validation::validate_tool_arguments;
use pi_host::{AbortSignal, LocalBoxFuture, PiError, PiResult};
use serde_json::{Map, Value};

use crate::stream_fn::get_default_stream_fn;
use crate::types::{
	AfterToolCallContext, AfterToolCallFn, AgentContext, AgentEvent, AgentLoopConfig, AgentMessage, AgentTool,
	AgentToolCall, AgentToolCallOutcome, AgentToolResult, AgentToolUpdateCallback, AgentTurnContext, AgentTurnDecision,
	BeforeToolCallContext, BeforeToolCallFn, PrepareRequestContext, StreamFn, ThinkingLevel, ToolExecutionMode,
};

/// Receives each event of a loop run. A rejected emit ends the run with that error.
pub type AgentEventSink = Rc<dyn Fn(AgentEvent) -> LocalBoxFuture<'static, PiResult<()>>>;

/// Request identity of agent loops: pi-ai's defaults with provider attribution to pi. `AgentLoopConfig.identity`
/// overrides it field by field.
pub static DEFAULT_AGENT_IDENTITY: LazyLock<ClientIdentity> = LazyLock::new(|| ClientIdentity {
	attribution: Some(Some(PI_ATTRIBUTION.clone())),
	..ClientIdentity::default()
});

/// Start an agent loop with a new prompt message.
/// The prompt is added to the context and events are emitted for it.
pub fn agent_loop(
	prompts: Vec<AgentMessage>,
	context: AgentContext,
	config: AgentLoopConfig,
	signal: Option<AbortSignal>,
	stream_fn: Option<StreamFn>,
) -> EventStream<AgentEvent, Vec<AgentMessage>> {
	let stream = create_agent_stream();

	let sink = stream.clone();
	let output = stream.clone();
	pi_host::spawn_local(async move {
		let emit: AgentEventSink = Rc::new(move |event| {
			sink.push(event);
			Box::pin(async { Ok(()) })
		});
		if let Ok(messages) = run_agent_loop(prompts, context, config, emit, signal, stream_fn).await {
			output.end(Some(messages));
		}
	});

	stream
}

/// Continue an agent loop from the current context without adding a new message.
/// Used for retries - context already has user message or tool results.
///
/// **Important:** The last message in context must convert to a `user` or `toolResult` message
/// via `convertToLlm`. If it doesn't, the LLM provider will reject the request.
/// This cannot be validated here since `convertToLlm` is only called once per turn.
pub fn agent_loop_continue(
	context: AgentContext,
	config: AgentLoopConfig,
	signal: Option<AbortSignal>,
	stream_fn: Option<StreamFn>,
) -> PiResult<EventStream<AgentEvent, Vec<AgentMessage>>> {
	check_continuation(&context)?;

	let stream = create_agent_stream();

	let sink = stream.clone();
	let output = stream.clone();
	pi_host::spawn_local(async move {
		let emit: AgentEventSink = Rc::new(move |event| {
			sink.push(event);
			Box::pin(async { Ok(()) })
		});
		if let Ok(messages) = run_agent_loop_continue(context, config, emit, signal, stream_fn).await {
			output.end(Some(messages));
		}
	});

	Ok(stream)
}

/// Run a prompt through the loop, emitting every event through `emit`, and return the messages the run added.
pub async fn run_agent_loop(
	prompts: Vec<AgentMessage>,
	context: AgentContext,
	config: AgentLoopConfig,
	emit: AgentEventSink,
	signal: Option<AbortSignal>,
	stream_fn: Option<StreamFn>,
) -> PiResult<Vec<AgentMessage>> {
	let initial_messages = declare_tool_changes(&context, prompts);
	let mut new_messages = initial_messages.clone();
	let mut current_context = context;
	current_context.messages.extend(initial_messages.iter().cloned());

	emit(AgentEvent::AgentStart).await?;
	emit(AgentEvent::TurnStart).await?;
	for message in initial_messages {
		emit(AgentEvent::MessageStart {
			message: message.clone(),
		})
		.await?;
		emit(AgentEvent::MessageEnd { message }).await?;
	}

	let stream_fn = match stream_fn {
		Some(stream_fn) => stream_fn,
		None => get_default_stream_fn()?,
	};
	run_loop(current_context, &mut new_messages, config, signal, &emit, stream_fn).await?;
	Ok(new_messages)
}

/// Continue the loop from `context` without new messages, emitting every event through `emit`, and return the
/// messages the run added.
pub async fn run_agent_loop_continue(
	context: AgentContext,
	config: AgentLoopConfig,
	emit: AgentEventSink,
	signal: Option<AbortSignal>,
	stream_fn: Option<StreamFn>,
) -> PiResult<Vec<AgentMessage>> {
	check_continuation(&context)?;

	let mut new_messages: Vec<AgentMessage> = Vec::new();

	emit(AgentEvent::AgentStart).await?;
	emit(AgentEvent::TurnStart).await?;

	let stream_fn = match stream_fn {
		Some(stream_fn) => stream_fn,
		None => get_default_stream_fn()?,
	};
	run_loop(context, &mut new_messages, config, signal, &emit, stream_fn).await?;
	Ok(new_messages)
}

fn check_continuation(context: &AgentContext) -> PiResult<()> {
	let Some(last) = context.messages.last() else {
		return Err(PiError::new("Cannot continue: no messages in context"));
	};
	if last.role() == "assistant" {
		return Err(PiError::new("Cannot continue from message role: assistant"));
	}
	Ok(())
}

fn create_agent_stream() -> EventStream<AgentEvent, Vec<AgentMessage>> {
	EventStream::new(
		|event: &AgentEvent| matches!(event, AgentEvent::AgentEnd { .. }),
		|event: &AgentEvent| match event {
			AgentEvent::AgentEnd { messages } => messages.clone(),
			_ => Vec::new(),
		},
	)
}

/// `reasoning` for a thinking level: `"off"` is no reasoning.
pub(crate) fn reasoning_for(level: ThinkingLevel) -> Option<ReasoningLevel> {
	match level {
		ModelThinkingLevel::Off => None,
		ModelThinkingLevel::Minimal => Some(ReasoningLevel::Minimal),
		ModelThinkingLevel::Low => Some(ReasoningLevel::Low),
		ModelThinkingLevel::Medium => Some(ReasoningLevel::Medium),
		ModelThinkingLevel::High => Some(ReasoningLevel::High),
		ModelThinkingLevel::Xhigh => Some(ReasoningLevel::Xhigh),
		ModelThinkingLevel::Max => Some(ReasoningLevel::Max),
	}
}

/// `config.reasoning ?? "off"`.
fn thinking_level_of(config: &AgentLoopConfig) -> ThinkingLevel {
	config
		.reasoning
		.map_or(ModelThinkingLevel::Off, ModelThinkingLevel::from)
}

async fn poll_messages(source: &Option<crate::types::GetMessagesFn>) -> PiResult<Vec<AgentMessage>> {
	match source {
		Some(source) => source().await,
		None => Ok(Vec::new()),
	}
}

/// Main loop logic shared by agentLoop and agentLoopContinue.
async fn run_loop(
	initial_context: AgentContext,
	new_messages: &mut Vec<AgentMessage>,
	initial_config: AgentLoopConfig,
	signal: Option<AbortSignal>,
	emit: &AgentEventSink,
	stream_function: StreamFn,
) -> PiResult<()> {
	let mut current_context = initial_context;
	let mut config = initial_config;
	// The message and tool results of the last completed turn. Its context and new messages are the live ones: the
	// loop changes neither between finishing a turn and preparing the next.
	let mut last_completed_turn: Option<(AssistantMessage, Vec<ToolResultMessage>)> = None;
	let mut explicit_continuation = false;
	// Check for steering messages at start (user may have typed while waiting)
	let mut pending_messages = poll_messages(&config.get_steering_messages).await?;

	// Outer loop: continues when queued follow-up messages arrive after agent would stop
	loop {
		let mut has_more_tool_calls = true;

		// Inner loop: process tool calls and steering messages
		while has_more_tool_calls || !pending_messages.is_empty() {
			let mut prepared_messages: Vec<AgentMessage> = Vec::new();
			if let Some((message, tool_results)) = &last_completed_turn {
				let next_turn_snapshot = match &config.prepare_next_turn {
					Some(prepare_next_turn) => {
						prepare_next_turn(AgentTurnContext {
							message: message.clone(),
							tool_results: tool_results.clone(),
							context: current_context.clone(),
							new_messages: new_messages.clone(),
						})
						.await?
					}
					None => None,
				};
				if let Some(next_turn_snapshot) = next_turn_snapshot {
					if let Some(context) = next_turn_snapshot.context {
						current_context = context;
					}
					prepared_messages = next_turn_snapshot.messages.unwrap_or_default();
					if let Some(model) = next_turn_snapshot.model {
						config.model = model;
					}
					if let Some(thinking_level) = next_turn_snapshot.thinking_level {
						config.reasoning = reasoning_for(thinking_level);
					}
				}
				// Preparation can be long-running (for example, compaction). Pick up steering
				// queued while it ran. Only poll again if the earlier poll returned nothing;
				// otherwise one-at-a-time mode would deliver two messages in this turn.
				if pending_messages.is_empty() {
					pending_messages = poll_messages(&config.get_steering_messages).await?;
				}
				emit(AgentEvent::TurnStart).await?;
			}

			// Process prepared and queued messages before the next assistant response.
			prepared_messages.append(&mut pending_messages);
			for message in declare_tool_changes(&current_context, prepared_messages) {
				emit(AgentEvent::MessageStart {
					message: message.clone(),
				})
				.await?;
				emit(AgentEvent::MessageEnd {
					message: message.clone(),
				})
				.await?;
				current_context.messages.push(message.clone());
				new_messages.push(message);
			}

			if let Some(prepare_request) = &config.prepare_request {
				let request_update = prepare_request(
					PrepareRequestContext {
						context: current_context.clone(),
						model: config.model.clone(),
						thinking_level: thinking_level_of(&config),
					},
					signal.clone(),
				)
				.await?;
				if let Some(request_update) = request_update {
					if let Some(context) = request_update.context {
						current_context = context;
					}
					if let Some(model) = request_update.model {
						config.model = model;
					}
					if let Some(thinking_level) = request_update.thinking_level {
						config.reasoning = reasoning_for(thinking_level);
					}
				}
			}

			// Stream assistant response
			let message =
				stream_assistant_response(&mut current_context, &config, signal.clone(), emit, &stream_function)
					.await?;
			new_messages.push(message.clone().into());

			if matches!(message.stop_reason, StopReason::Error | StopReason::Aborted) {
				if let Some(finish_turn) = &config.finish_turn {
					finish_turn(
						AgentTurnContext {
							message: message.clone(),
							tool_results: Vec::new(),
							context: current_context.clone(),
							new_messages: new_messages.clone(),
						},
						signal.clone(),
					)
					.await?;
				}
				emit(AgentEvent::TurnEnd {
					message: message.into(),
					tool_results: Vec::new(),
				})
				.await?;
				emit(AgentEvent::AgentEnd {
					messages: new_messages.clone(),
				})
				.await?;
				return Ok(());
			}

			// Check for tool calls
			let tool_calls = tool_calls_of(&message);

			let mut tool_results: Vec<ToolResultMessage> = Vec::new();
			has_more_tool_calls = false;
			if !tool_calls.is_empty() {
				// A "length" stop means the output was cut off by the token limit, so
				// every tool call in the message may carry truncated arguments. Fail
				// them all instead of executing potentially borked calls.
				let executed_tool_batch = if message.stop_reason == StopReason::Length {
					fail_tool_calls_from_truncated_message(&tool_calls, emit).await?
				} else {
					execute_tool_calls(&current_context, &message, tool_calls, &config, signal.clone(), emit).await?
				};
				tool_results.extend(executed_tool_batch.messages);
				has_more_tool_calls = !executed_tool_batch.terminate;

				for result in &tool_results {
					current_context.messages.push(result.clone().into());
					new_messages.push(result.clone().into());
				}
			}

			let decision = match &config.finish_turn {
				Some(finish_turn) => {
					finish_turn(
						AgentTurnContext {
							message: message.clone(),
							tool_results: tool_results.clone(),
							context: current_context.clone(),
							new_messages: new_messages.clone(),
						},
						signal.clone(),
					)
					.await?
				}
				None => None,
			};
			last_completed_turn = Some((message.clone(), tool_results.clone()));
			emit(AgentEvent::TurnEnd {
				message: message.into(),
				tool_results,
			})
			.await?;

			if decision == Some(AgentTurnDecision::End) {
				emit(AgentEvent::AgentEnd {
					messages: new_messages.clone(),
				})
				.await?;
				return Ok(());
			}

			explicit_continuation = decision == Some(AgentTurnDecision::Continue);
			pending_messages = poll_messages(&config.get_steering_messages).await?;
			if has_more_tool_calls || !pending_messages.is_empty() {
				explicit_continuation = false;
			}
		}

		// Agent would stop here. Check for follow-up messages.
		let follow_up_messages = poll_messages(&config.get_follow_up_messages).await?;
		if !follow_up_messages.is_empty() {
			// Set as pending so inner loop processes them
			explicit_continuation = false;
			pending_messages = follow_up_messages;
			continue;
		}

		// No natural request was selected, so fulfill the continuation decision with one context-only turn.
		if explicit_continuation {
			explicit_continuation = false;
			continue;
		}

		// No more messages, exit
		break;
	}

	emit(AgentEvent::AgentEnd {
		messages: new_messages.clone(),
	})
	.await?;
	Ok(())
}

fn tool_calls_of(message: &AssistantMessage) -> Vec<AgentToolCall> {
	message
		.content
		.iter()
		.filter_map(|content| match content {
			AssistantContent::ToolCall(tool_call) => Some(tool_call.clone()),
			_ => None,
		})
		.collect()
}

/// Declare tool loadout changes to the model.
///
/// `context.tools` is what the runtime can execute; the transcript's system messages declare
/// what the model may call. Before each request the difference becomes `toolsAdded` and
/// `toolsRemoved` on a system message. When a pending system message exists, its tool fields
/// are treated as intent and replaced with the delta between the committed transcript and
/// the executable set, so replay always yields exactly `context.tools`. Otherwise a new
/// system message is inserted before the first non-system pending message.
fn declare_tool_changes(context: &AgentContext, pending_messages: Vec<AgentMessage>) -> Vec<AgentMessage> {
	let system_index = pending_messages
		.iter()
		.rposition(|message| message.as_system_message().is_some());
	// The system messages of `[...context.messages, ...baseline]`: the replay reads nothing else.
	let mut declared: Vec<SystemMessage> = context
		.messages
		.iter()
		.filter_map(TranscriptMessage::as_system_message)
		.cloned()
		.collect();
	for (index, message) in pending_messages.iter().enumerate() {
		if let Some(message) = message.as_system_message() {
			declared.push(if Some(index) == system_index {
				with_tool_changes(message, &ToolStateChanges::default())
			} else {
				message.clone()
			});
		}
	}
	let executable: Vec<_> = context
		.tools
		.iter()
		.flatten()
		.map(|tool| to_tool_declaration(&tool.tool))
		.collect();
	let changes = get_tool_state_changes(&get_current_tools(&declared), &executable);
	let unchanged = changes.tools_added.is_empty() && changes.tools_removed.is_empty();

	if let Some(system_index) = system_index {
		let pending = pending_messages[system_index]
			.as_system_message()
			.expect("systemIndex points at a system message");
		// Keep the caller's message object when it already declares no tool changes.
		if unchanged
			&& pending.tools_added.as_ref().is_none_or(Vec::is_empty)
			&& pending.tools_removed.as_ref().is_none_or(Vec::is_empty)
		{
			return pending_messages;
		}
		let update = with_tool_changes(pending, &changes);
		let mut messages = pending_messages;
		messages[system_index] = update.into();
		return messages;
	}
	if unchanged {
		return pending_messages;
	}
	let update = with_tool_changes(
		&SystemMessage {
			content: SystemMessageContent::Text(String::new()),
			timestamp: pi_host::now_ms(),
			..SystemMessage::default()
		},
		&changes,
	);
	let index = pending_messages
		.iter()
		.position(|message| message.as_system_message().is_none())
		.unwrap_or(pending_messages.len());
	let mut messages = pending_messages;
	messages.insert(index, update.into());
	messages
}

/// Copy a system message with its tool fields replaced by `changes`; empty lists omit the field.
fn with_tool_changes(message: &SystemMessage, changes: &ToolStateChanges) -> SystemMessage {
	SystemMessage {
		tools_added: (!changes.tools_added.is_empty()).then(|| changes.tools_added.clone()),
		tools_removed: (!changes.tools_removed.is_empty()).then(|| changes.tools_removed.clone()),
		..message.clone()
	}
}

/// Stream an assistant response from the LLM.
/// This is where AgentMessage[] gets transformed to Message[] for the LLM.
async fn stream_assistant_response(
	context: &mut AgentContext,
	config: &AgentLoopConfig,
	signal: Option<AbortSignal>,
	emit: &AgentEventSink,
	stream_function: &StreamFn,
) -> PiResult<AssistantMessage> {
	// Apply context transform if configured (AgentMessage[] → AgentMessage[])
	let messages = match &config.transform_context {
		Some(transform_context) => transform_context(context.messages.clone(), signal.clone()).await?,
		None => context.messages.clone(),
	};

	// Convert to LLM-compatible messages (AgentMessage[] → Message[])
	let llm_messages = (config.convert_to_llm)(messages).await?;

	let llm_context = normalize_context(&Context {
		messages: llm_messages,
		..Context::default()
	});

	// Resolve API key (important for expiring tokens)
	let dynamic_api_key = match &config.get_api_key {
		Some(get_api_key) => get_api_key(config.model.provider.to_string()).await?,
		None => None,
	};
	let resolved_api_key = dynamic_api_key
		.filter(|key| !key.is_empty())
		.or_else(|| config.api_key.clone());

	let mut options = config.base.clone();
	options.api_key = resolved_api_key;
	options.identity = merge_client_identity(&[Some(&DEFAULT_AGENT_IDENTITY), config.identity.as_ref()]);
	options.signal = signal;
	let response = stream_function(&config.model, &llm_context, Some(options)).await?;
	// Record the requested level, whichever stream function answered.
	let thinking_level = thinking_level_of(config);

	let mut partial_message: Option<SharedAssistantMessage> = None;
	// TS pushes every `start` partial into the context and replaces only the last one with the final message.
	let mut earlier_partials: Vec<SharedAssistantMessage> = Vec::new();

	while let Some(event) = response.next().await {
		match &event {
			AssistantMessageEvent::Start { partial } => {
				if let Some(previous) = partial_message.replace(partial.clone()) {
					earlier_partials.push(previous);
				}
				let message = partial.borrow().clone();
				emit(AgentEvent::MessageStart {
					message: message.into(),
				})
				.await?;
			}

			AssistantMessageEvent::TextStart { partial, .. }
			| AssistantMessageEvent::TextDelta { partial, .. }
			| AssistantMessageEvent::TextEnd { partial, .. }
			| AssistantMessageEvent::ThinkingStart { partial, .. }
			| AssistantMessageEvent::ThinkingDelta { partial, .. }
			| AssistantMessageEvent::ThinkingEnd { partial, .. }
			| AssistantMessageEvent::ToolcallStart { partial, .. }
			| AssistantMessageEvent::ToolcallDelta { partial, .. }
			| AssistantMessageEvent::ToolcallEnd { partial, .. } => {
				if partial_message.is_some() {
					partial_message = Some(partial.clone());
					emit(AgentEvent::MessageUpdate {
						message: partial.clone(),
						assistant_message_event: event.clone(),
					})
					.await?;
				}
			}

			AssistantMessageEvent::Done { .. } | AssistantMessageEvent::Error { .. } => {
				return finish_response(
					context,
					&response,
					thinking_level,
					partial_message.is_some(),
					&earlier_partials,
					emit,
				)
				.await;
			}
		}
	}

	finish_response(
		context,
		&response,
		thinking_level,
		partial_message.is_some(),
		&earlier_partials,
		emit,
	)
	.await
}

async fn finish_response(
	context: &mut AgentContext,
	response: &AssistantMessageEventStream,
	thinking_level: ThinkingLevel,
	added_partial: bool,
	earlier_partials: &[SharedAssistantMessage],
	emit: &AgentEventSink,
) -> PiResult<AssistantMessage> {
	let mut final_message = response
		.result()
		.await
		.expect("the stream function's stream resolves its result once it ends");
	final_message.thinking_level = Some(thinking_level);
	context
		.messages
		.extend(earlier_partials.iter().map(|partial| partial.borrow().clone().into()));
	context.messages.push(final_message.clone().into());
	if !added_partial {
		emit(AgentEvent::MessageStart {
			message: final_message.clone().into(),
		})
		.await?;
	}
	emit(AgentEvent::MessageEnd {
		message: final_message.clone().into(),
	})
	.await?;
	Ok(final_message)
}

/// Fail all tool calls from an assistant message that was truncated by the
/// output token limit. Streamed tool-call arguments are finalized with a
/// best-effort JSON salvage parser, so a truncated message can yield tool calls
/// whose arguments parse and validate but are silently incomplete. None of them
/// are safe to execute; report each as an error so the model can re-issue them.
async fn fail_tool_calls_from_truncated_message(
	tool_calls: &[AgentToolCall],
	emit: &AgentEventSink,
) -> PiResult<ExecutedToolCallBatch> {
	let mut messages: Vec<ToolResultMessage> = Vec::new();
	for tool_call in tool_calls {
		emit_tool_execution_start(tool_call, emit).await?;
		let finalized = FinalizedToolCallOutcome {
			tool_call: tool_call.clone(),
			result: create_error_tool_result(&format!(
				"Tool call \"{}\" was not executed: the response hit the output token limit, so its arguments may be truncated. Re-issue the tool call with complete arguments.",
				tool_call.name
			)),
			is_error: true,
			duration_ms: None,
		};
		emit_tool_execution_end(&finalized, emit).await?;
		let tool_result_message = create_tool_result_message(&finalized);
		emit_tool_result_message(&tool_result_message, emit).await?;
		messages.push(tool_result_message);
	}
	Ok(ExecutedToolCallBatch {
		messages,
		terminate: false,
	})
}

async fn emit_tool_execution_start(tool_call: &AgentToolCall, emit: &AgentEventSink) -> PiResult<()> {
	emit(AgentEvent::ToolExecutionStart {
		tool_call_id: tool_call.id.clone(),
		tool_name: tool_call.name.clone(),
		args: Value::Object(tool_call.arguments.clone()),
	})
	.await
}

/// Execute tool calls from an assistant message.
async fn execute_tool_calls(
	current_context: &AgentContext,
	assistant_message: &AssistantMessage,
	tool_calls: Vec<AgentToolCall>,
	config: &AgentLoopConfig,
	signal: Option<AbortSignal>,
	emit: &AgentEventSink,
) -> PiResult<ExecutedToolCallBatch> {
	let has_sequential_tool_call = tool_calls.iter().any(|tool_call| {
		current_context
			.tools
			.iter()
			.flatten()
			.find(|tool| tool.name == tool_call.name)
			.is_some_and(|tool| tool.execution_mode == Some(ToolExecutionMode::Sequential))
	});
	let hooks = ToolCallHooks {
		before_tool_call: config.before_tool_call.clone(),
		after_tool_call: config.after_tool_call.clone(),
	};
	if config.tool_execution == Some(ToolExecutionMode::Sequential) || has_sequential_tool_call {
		return execute_tool_calls_sequential(current_context, assistant_message, tool_calls, &hooks, signal, emit)
			.await;
	}
	execute_tool_calls_parallel(current_context, assistant_message, tool_calls, &hooks, signal, emit).await
}

struct ExecutedToolCallBatch {
	messages: Vec<ToolResultMessage>,
	terminate: bool,
}

fn is_aborted(signal: &Option<AbortSignal>) -> bool {
	signal.as_ref().is_some_and(AbortSignal::aborted)
}

async fn execute_tool_calls_sequential(
	current_context: &AgentContext,
	assistant_message: &AssistantMessage,
	tool_calls: Vec<AgentToolCall>,
	hooks: &ToolCallHooks,
	signal: Option<AbortSignal>,
	emit: &AgentEventSink,
) -> PiResult<ExecutedToolCallBatch> {
	let tools = current_context.tools.as_deref().unwrap_or_default();
	let mut finalized_calls: Vec<FinalizedToolCallOutcome> = Vec::new();
	let mut messages: Vec<ToolResultMessage> = Vec::new();

	for tool_call in tool_calls {
		emit_tool_execution_start(&tool_call, emit).await?;

		let preparation = prepare_tool_call(
			current_context,
			assistant_message,
			tool_call.clone(),
			hooks,
			signal.clone(),
			tools,
		)
		.await;
		let finalized = match preparation {
			Preparation::Immediate { result, is_error } => FinalizedToolCallOutcome {
				tool_call,
				result,
				is_error,
				duration_ms: None,
			},
			Preparation::Prepared(preparation) => {
				let executed = execute_prepared_tool_call(
					&preparation,
					signal.clone(),
					emit_tool_execution_update(&preparation.tool_call, emit),
				)
				.await?;
				finalize_executed_tool_call(
					current_context,
					assistant_message,
					preparation,
					executed,
					hooks,
					signal.clone(),
				)
				.await
			}
		};

		emit_tool_execution_end(&finalized, emit).await?;
		let tool_result_message = create_tool_result_message(&finalized);
		emit_tool_result_message(&tool_result_message, emit).await?;
		finalized_calls.push(finalized);
		messages.push(tool_result_message);

		if is_aborted(&signal) {
			break;
		}
	}

	Ok(ExecutedToolCallBatch {
		messages,
		terminate: should_terminate_tool_batch(&finalized_calls),
	})
}

async fn execute_tool_calls_parallel(
	current_context: &AgentContext,
	assistant_message: &AssistantMessage,
	tool_calls: Vec<AgentToolCall>,
	hooks: &ToolCallHooks,
	signal: Option<AbortSignal>,
	emit: &AgentEventSink,
) -> PiResult<ExecutedToolCallBatch> {
	let tools = current_context.tools.as_deref().unwrap_or_default();
	let mut finalized_calls: Vec<FinalizedToolCallEntry> = Vec::new();

	for tool_call in tool_calls {
		emit_tool_execution_start(&tool_call, emit).await?;

		let preparation = prepare_tool_call(
			current_context,
			assistant_message,
			tool_call.clone(),
			hooks,
			signal.clone(),
			tools,
		)
		.await;
		match preparation {
			Preparation::Immediate { result, is_error } => {
				let finalized = FinalizedToolCallOutcome {
					tool_call,
					result,
					is_error,
					duration_ms: None,
				};
				emit_tool_execution_end(&finalized, emit).await?;
				finalized_calls.push(FinalizedToolCallEntry::Finalized(finalized));
				if is_aborted(&signal) {
					break;
				}
				continue;
			}
			Preparation::Prepared(preparation) => finalized_calls.push(FinalizedToolCallEntry::Pending(preparation)),
		}
		if is_aborted(&signal) {
			break;
		}
	}

	// Each call settles on its own, so `tool_execution_end` follows completion order like the TS promise chains;
	// the results are put back in source order for the tool result messages.
	let call_count = finalized_calls.len();
	let mut running: FuturesUnordered<_> = finalized_calls
		.into_iter()
		.enumerate()
		.map(|(index, entry)| {
			let signal = signal.clone();
			async move {
				let preparation = match entry {
					FinalizedToolCallEntry::Finalized(finalized) => return Ok((index, finalized)),
					FinalizedToolCallEntry::Pending(preparation) => preparation,
				};
				if is_aborted(&signal) {
					let finalized = FinalizedToolCallOutcome {
						tool_call: preparation.tool_call,
						result: create_error_tool_result("Operation aborted"),
						is_error: true,
						duration_ms: None,
					};
					emit_tool_execution_end(&finalized, emit).await?;
					return Ok((index, finalized));
				}
				let executed = execute_prepared_tool_call(
					&preparation,
					signal.clone(),
					emit_tool_execution_update(&preparation.tool_call, emit),
				)
				.await?;
				let finalized = finalize_executed_tool_call(
					current_context,
					assistant_message,
					preparation,
					executed,
					hooks,
					signal,
				)
				.await;
				emit_tool_execution_end(&finalized, emit).await?;
				PiResult::Ok((index, finalized))
			}
		})
		.collect();
	let mut settled: Vec<Option<FinalizedToolCallOutcome>> = (0..call_count).map(|_| None).collect();
	while let Some(outcome) = running.next().await {
		let (index, finalized) = outcome?;
		settled[index] = Some(finalized);
	}
	let ordered_finalized_calls: Vec<FinalizedToolCallOutcome> = settled
		.into_iter()
		.map(|finalized| finalized.expect("every running tool call settles before the loop ends"))
		.collect();
	let mut messages: Vec<ToolResultMessage> = Vec::new();
	for finalized in &ordered_finalized_calls {
		let tool_result_message = create_tool_result_message(finalized);
		emit_tool_result_message(&tool_result_message, emit).await?;
		messages.push(tool_result_message);
	}

	Ok(ExecutedToolCallBatch {
		messages,
		terminate: should_terminate_tool_batch(&ordered_finalized_calls),
	})
}

struct PreparedToolCall<'a> {
	tool_call: AgentToolCall,
	tool: &'a AgentTool,
	args: Value,
}

enum Preparation<'a> {
	Prepared(PreparedToolCall<'a>),
	Immediate { result: AgentToolResult, is_error: bool },
}

struct ExecutedToolCallOutcome {
	result: AgentToolResult,
	is_error: bool,
	duration_ms: f64,
}

type FinalizedToolCallOutcome = AgentToolCallOutcome;

/// The `beforeToolCall` and `afterToolCall` hooks of [`AgentLoopConfig`].
#[derive(Clone, Default)]
pub struct ToolCallHooks {
	pub before_tool_call: Option<BeforeToolCallFn>,
	pub after_tool_call: Option<AfterToolCallFn>,
}

/// Receives a tool's partial results.
pub type ToolUpdateSink = Rc<dyn Fn(AgentToolResult) -> LocalBoxFuture<'static, PiResult<()>>>;

enum FinalizedToolCallEntry<'a> {
	Finalized(FinalizedToolCallOutcome),
	Pending(PreparedToolCall<'a>),
}

fn should_terminate_tool_batch(finalized_calls: &[FinalizedToolCallOutcome]) -> bool {
	!finalized_calls.is_empty()
		&& finalized_calls
			.iter()
			.all(|finalized| finalized.result.terminate == Some(true))
}

fn prepare_tool_call_arguments(tool: &AgentTool, tool_call: &AgentToolCall) -> PiResult<AgentToolCall> {
	let Some(prepare_arguments) = &tool.prepare_arguments else {
		return Ok(tool_call.clone());
	};
	let prepared_arguments = prepare_arguments(Value::Object(tool_call.arguments.clone()))?;
	Ok(AgentToolCall {
		arguments: prepared_arguments,
		..tool_call.clone()
	})
}

async fn prepare_tool_call<'a>(
	current_context: &AgentContext,
	assistant_message: &AssistantMessage,
	tool_call: AgentToolCall,
	hooks: &ToolCallHooks,
	signal: Option<AbortSignal>,
	tools: &'a [AgentTool],
) -> Preparation<'a> {
	let Some(tool) = tools.iter().find(|tool| tool.name == tool_call.name) else {
		return Preparation::Immediate {
			result: create_error_tool_result(&format!("Tool {} not found", tool_call.name)),
			is_error: true,
		};
	};

	match prepare_found_tool_call(current_context, assistant_message, tool_call, tool, hooks, signal).await {
		Ok(preparation) => preparation,
		Err(error) => Preparation::Immediate {
			result: create_error_tool_result(&error.message),
			is_error: true,
		},
	}
}

/// The `try` block of `prepareToolCall`.
async fn prepare_found_tool_call<'a>(
	current_context: &AgentContext,
	assistant_message: &AssistantMessage,
	tool_call: AgentToolCall,
	tool: &'a AgentTool,
	hooks: &ToolCallHooks,
	signal: Option<AbortSignal>,
) -> PiResult<Preparation<'a>> {
	let prepared_tool_call = prepare_tool_call_arguments(tool, &tool_call)?;
	let mut validated_args = validate_tool_arguments(&tool.tool, &prepared_tool_call)?;
	if let Some(before_tool_call) = &hooks.before_tool_call {
		let args = Rc::new(RefCell::new(validated_args));
		let before_result = before_tool_call(
			BeforeToolCallContext {
				assistant_message: assistant_message.clone(),
				tool_call: tool_call.clone(),
				args: args.clone(),
				context: current_context.clone(),
			},
			signal.clone(),
		)
		.await?;
		validated_args = args.borrow().clone();
		if is_aborted(&signal) {
			return Ok(Preparation::Immediate {
				result: create_error_tool_result("Operation aborted"),
				is_error: true,
			});
		}
		if let Some(before_result) = before_result
			&& before_result.block == Some(true)
		{
			let reason = before_result.reason.filter(|reason| !reason.is_empty());
			let mut result = create_error_tool_result(reason.as_deref().unwrap_or("Tool execution was blocked"));
			if before_result.terminate == Some(true) {
				result.terminate = Some(true);
			}
			return Ok(Preparation::Immediate { result, is_error: true });
		}
	}
	if is_aborted(&signal) {
		return Ok(Preparation::Immediate {
			result: create_error_tool_result("Operation aborted"),
			is_error: true,
		});
	}
	Ok(Preparation::Prepared(PreparedToolCall {
		tool_call,
		tool,
		args: validated_args,
	}))
}

fn emit_tool_execution_update(tool_call: &AgentToolCall, emit: &AgentEventSink) -> ToolUpdateSink {
	let tool_call_id = tool_call.id.clone();
	let tool_name = tool_call.name.clone();
	let args = Value::Object(tool_call.arguments.clone());
	let emit = emit.clone();
	Rc::new(move |partial_result| {
		emit(AgentEvent::ToolExecutionUpdate {
			tool_call_id: tool_call_id.clone(),
			tool_name: tool_name.clone(),
			args: args.clone(),
			partial_result,
		})
	})
}

/// Options for [`run_tool_call`].
#[derive(Clone)]
pub struct RunToolCallOptions {
	pub base: ToolCallHooks,
	/// Tools the call resolves against.
	pub tools: Vec<AgentTool>,
	/// Passed to the hooks as the message that issued the call.
	pub assistant_message: AssistantMessage,
	/// Passed to the hooks as the current agent context.
	pub context: AgentContext,
	pub signal: Option<AbortSignal>,
	pub on_update: Option<ToolUpdateSink>,
}

impl std::ops::Deref for RunToolCallOptions {
	type Target = ToolCallHooks;

	fn deref(&self) -> &Self::Target {
		&self.base
	}
}

impl std::ops::DerefMut for RunToolCallOptions {
	fn deref_mut(&mut self) -> &mut Self::Target {
		&mut self.base
	}
}

/// Run one tool call through the same steps as a model-issued call: argument preparation, schema
/// validation, `beforeToolCall`, execution, and `afterToolCall`. Emits no events and adds no
/// messages. Tools that call other tools use this so the hooks (for example permission checks)
/// apply to those calls too.
///
/// Never rejects for tool failures: unknown tools, validation errors, blocked calls, and thrown
/// errors come back as `isError: true`. It rejects only when `onUpdate` does.
pub async fn run_tool_call(tool_call: AgentToolCall, options: RunToolCallOptions) -> PiResult<AgentToolCallOutcome> {
	let RunToolCallOptions {
		base: hooks,
		tools,
		assistant_message,
		context,
		signal,
		on_update,
	} = options;
	let preparation = prepare_tool_call(
		&context,
		&assistant_message,
		tool_call.clone(),
		&hooks,
		signal.clone(),
		&tools,
	)
	.await;
	let preparation = match preparation {
		Preparation::Immediate { result, is_error } => {
			return Ok(AgentToolCallOutcome {
				tool_call,
				result,
				is_error,
				duration_ms: None,
			});
		}
		Preparation::Prepared(preparation) => preparation,
	};
	let on_update = on_update.unwrap_or_else(|| Rc::new(|_| Box::pin(async { Ok(()) })));
	let executed = execute_prepared_tool_call(&preparation, signal.clone(), on_update).await?;
	Ok(finalize_executed_tool_call(&context, &assistant_message, preparation, executed, &hooks, signal).await)
}

async fn execute_prepared_tool_call(
	prepared: &PreparedToolCall<'_>,
	signal: Option<AbortSignal>,
	on_update: ToolUpdateSink,
) -> PiResult<ExecutedToolCallOutcome> {
	let update_events: Rc<RefCell<Vec<oneshot::Receiver<PiResult<()>>>>> = Rc::new(RefCell::new(Vec::new()));
	let accepting_updates = Rc::new(Cell::new(true));
	let started_at = pi_host::monotonic_ms();
	let elapsed = || (pi_host::monotonic_ms() - started_at).round();

	let callback: AgentToolUpdateCallback = {
		let update_events = update_events.clone();
		let accepting_updates = accepting_updates.clone();
		Rc::new(move |partial_result| {
			if !accepting_updates.get() {
				return;
			}
			let (resolve, update_event) = oneshot::channel();
			let emitted = on_update(partial_result);
			pi_host::spawn_local(async move {
				let _ = resolve.send(emitted.await);
			});
			update_events.borrow_mut().push(update_event);
		})
	};
	let outcome = (prepared.tool.execute)(
		prepared.tool_call.id.clone(),
		prepared.args.clone(),
		signal,
		Some(callback),
	)
	.await;
	let duration_ms = elapsed();
	accepting_updates.set(false);
	let pending_updates = std::mem::take(&mut *update_events.borrow_mut());
	for update_event in pending_updates {
		if let Ok(Err(error)) = update_event.await {
			return Err(error);
		}
	}
	Ok(match outcome {
		Ok(result) => ExecutedToolCallOutcome {
			is_error: result.is_error == Some(true),
			result,
			duration_ms,
		},
		Err(error) => ExecutedToolCallOutcome {
			result: create_error_tool_result(&error.message),
			is_error: true,
			duration_ms,
		},
	})
}

async fn finalize_executed_tool_call(
	current_context: &AgentContext,
	assistant_message: &AssistantMessage,
	prepared: PreparedToolCall<'_>,
	executed: ExecutedToolCallOutcome,
	hooks: &ToolCallHooks,
	signal: Option<AbortSignal>,
) -> FinalizedToolCallOutcome {
	let mut result = executed.result;
	let mut is_error = executed.is_error;

	if let Some(after_tool_call) = &hooks.after_tool_call {
		let after_result = after_tool_call(
			AfterToolCallContext {
				assistant_message: assistant_message.clone(),
				tool_call: prepared.tool_call.clone(),
				args: prepared.args,
				result: result.clone(),
				is_error,
				context: current_context.clone(),
			},
			signal,
		)
		.await;
		match after_result {
			Ok(Some(after_result)) => {
				// Structured content not replaced along with the content may no longer match it.
				let structured_content = after_result.structured_content.filter(|value| !value.is_null()).or(
					if after_result.content.is_some() {
						None
					} else {
						result.structured_content.take()
					},
				);
				result = AgentToolResult {
					content: after_result.content.unwrap_or(result.content),
					details: after_result.details.filter(|value| !value.is_null()).or(result.details),
					usage: after_result.usage.or(result.usage),
					terminate: after_result.terminate.or(result.terminate),
					structured_content,
					..result
				};
				is_error = after_result.is_error.unwrap_or(is_error);
			}
			Ok(None) => {}
			Err(error) => {
				result = create_error_tool_result(&error.message);
				is_error = true;
			}
		}
	}

	FinalizedToolCallOutcome {
		tool_call: prepared.tool_call,
		result,
		is_error,
		duration_ms: Some(executed.duration_ms),
	}
}

fn create_error_tool_result(message: &str) -> AgentToolResult {
	AgentToolResult {
		content: vec![UserContent::Text(TextContent {
			text: message.to_string(),
			..TextContent::default()
		})],
		details: Some(Value::Object(Map::new())),
		..AgentToolResult::default()
	}
}

async fn emit_tool_execution_end(finalized: &FinalizedToolCallOutcome, emit: &AgentEventSink) -> PiResult<()> {
	emit(AgentEvent::ToolExecutionEnd {
		tool_call_id: finalized.tool_call.id.clone(),
		tool_name: finalized.tool_call.name.clone(),
		result: finalized.result.clone(),
		is_error: finalized.is_error,
		duration_ms: finalized.duration_ms,
	})
	.await
}

fn create_tool_result_message(finalized: &FinalizedToolCallOutcome) -> ToolResultMessage {
	ToolResultMessage {
		tool_call_id: finalized.tool_call.id.clone(),
		tool_name: finalized.tool_call.name.clone(),
		// Untyped tools (JS extensions) can return results without content; normalize
		// so the null never enters session history or provider payloads.
		content: finalized.result.content.clone(),
		details: finalized.result.details.clone(),
		usage: finalized.result.usage.clone(),
		is_error: finalized.is_error,
		duration_ms: finalized.duration_ms,
		timestamp: pi_host::now_ms(),
		..ToolResultMessage::default()
	}
}

async fn emit_tool_result_message(tool_result_message: &ToolResultMessage, emit: &AgentEventSink) -> PiResult<()> {
	emit(AgentEvent::MessageStart {
		message: tool_result_message.clone().into(),
	})
	.await?;
	emit(AgentEvent::MessageEnd {
		message: tool_result_message.clone().into(),
	})
	.await
}
