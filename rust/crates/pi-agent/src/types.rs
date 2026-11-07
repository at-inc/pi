//! Port of `packages/agent/src/types.ts`.
//! PORT-STATUS: ported
//!
//! Dropped type-level helpers with no runtime effect: `AgentToolParams`, `IsAny`. Generic types whose parameters only
//! narrow a field (`AgentTool<TParameters, TDetails>`, `AgentToolResult<T>`, `AgentToolUpdateCallback<T>`) are one type
//! with `serde_json::Value` for the narrowed field. `CustomAgentMessages` declaration merging is replaced by
//! `AgentMessage::Custom`: an app's custom messages pass through the core as raw JSON.
//!
//! Rust differences:
//! - `message_update` carries the provider's live `SharedAssistantMessage`. TS passes a shallow copy whose `content`
//!   array is the live array; a Rust copy would be a deep copy per delta, quadratic in the response length.
//! - `BeforeToolCallContext.args` is shared (`Rc<RefCell<Value>>`): a hook that changes the arguments changes what the
//!   tool executes, as mutating the TS object does.
//! - Hook contexts (`context`, `assistantMessage`) are snapshots taken when the hook runs, where TS passes the live
//!   loop objects.
//! - `AgentState` is a struct; the accessor properties `tools` and `messages` are methods.

use std::cell::RefCell;
use std::ops::{Deref, DerefMut};
use std::rc::Rc;

use indexmap::IndexSet;
use pi_ai::types::{
	AssistantMessage, AssistantMessageEvent, Message, Model, ModelThinkingLevel, SharedAssistantMessage,
	SimpleStreamOptions, SystemMessage, Tool, ToolCall, ToolResultMessage, TranscriptContext, Usage, UserContent,
	UserMessage,
};
use pi_ai::utils::event_stream::AssistantMessageEventStream;
use pi_ai::utils::transcript::TranscriptMessage;
use pi_host::{AbortSignal, LocalBoxFuture, PiResult};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::{Map, Value};

/// Stream function used by the agent loop. `Models.streamSimple` satisfies
/// this shape.
///
/// The loop passes a normalized transcript: the system prompt and tool
/// declarations are carried by the transcript's system messages, never by
/// `context.systemPrompt` or `context.tools`.
///
/// Contract:
/// - Must not throw or return a rejected promise for request/model/runtime failures.
/// - Must return an AssistantMessageEventStream.
/// - Failures must be encoded in the returned stream via protocol events and a
///   final AssistantMessage with stopReason "error" or "aborted" and errorMessage.
pub type StreamFn = Rc<
	dyn Fn(
		&Model,
		&TranscriptContext,
		Option<SimpleStreamOptions>,
	) -> LocalBoxFuture<'static, PiResult<AssistantMessageEventStream>>,
>;

/// Configuration for how tool calls from a single assistant message are executed.
///
/// - "sequential": each tool call is prepared, executed, and finalized before the next one starts.
/// - "parallel": tool calls are prepared sequentially, then allowed tools execute concurrently.
///   `tool_execution_end` is emitted in tool completion order after each tool is finalized,
///   while tool-result message artifacts are emitted later in assistant source order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ToolExecutionMode {
	#[serde(rename = "sequential")]
	Sequential,
	#[serde(rename = "parallel")]
	Parallel,
}

/// Controls how many queued user messages are injected when the agent loop reaches a queue drain point.
///
/// - "all": drain and inject every queued message at that point.
/// - "one-at-a-time": drain and inject only the oldest queued message, leaving the rest queued for later drain points.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum QueueMode {
	#[serde(rename = "all")]
	All,
	#[serde(rename = "one-at-a-time")]
	OneAtATime,
}

/// A single tool call content block emitted by an assistant message.
pub type AgentToolCall = ToolCall;

/// Result returned from `beforeToolCall`.
///
/// Returning `{ block: true }` prevents the tool from executing. The loop emits an error tool result instead.
/// `reason` becomes the text shown in that error result. If omitted, a default blocked message is used.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BeforeToolCallResult {
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub block: Option<bool>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub reason: Option<String>,
	/// Hint that the agent should stop after the current tool batch when this call is blocked.
	/// Early termination only happens when every finalized tool result in the batch sets this to true.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub terminate: Option<bool>,
}

/// Partial override returned from `afterToolCall`.
///
/// Merge semantics are field-by-field:
/// - `content`: if provided, replaces the tool result content array in full
/// - `details`: if provided, replaces the tool result details value in full
/// - `isError`: if provided, replaces the tool result error flag
/// - `usage`: if provided, replaces the tool result usage
/// - `terminate`: if provided, replaces the early-termination hint
/// - `structuredContent`: if provided, replaces the structured content. If `content` is provided
///   without it, the structured content is dropped, because it may no longer match the content.
///   Return it along with `content` to keep it.
///
/// Other omitted fields keep the original executed tool result values.
/// There is no deep merge for `content`, `details`, or `usage`. A `null` `details` or `structuredContent` counts as
/// omitted, like the TS `??`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AfterToolCallResult {
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub content: Option<Vec<UserContent>>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub details: Option<Value>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub structured_content: Option<Value>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub is_error: Option<bool>,
	/// Usage from the final tool execution itself, if available. Not used for main LLM context accounting.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub usage: Option<Usage>,
	/// Hint that the agent should stop after the current tool batch.
	/// Early termination only happens when every finalized tool result in the batch sets this to true.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub terminate: Option<bool>,
}

/// Context passed to `beforeToolCall`.
#[derive(Clone, Debug)]
pub struct BeforeToolCallContext {
	/// The assistant message that requested the tool call.
	pub assistant_message: AssistantMessage,
	/// The raw tool call block from `assistantMessage.content`.
	pub tool_call: AgentToolCall,
	/// Validated tool arguments for the target tool schema. Shared with the execution: a hook that changes them
	/// changes what the tool receives, without revalidation.
	pub args: Rc<RefCell<Value>>,
	/// Current agent context at the time the tool call is prepared.
	pub context: AgentContext,
}

/// Context passed to `afterToolCall`.
#[derive(Clone, Debug)]
pub struct AfterToolCallContext {
	/// The assistant message that requested the tool call.
	pub assistant_message: AssistantMessage,
	/// The raw tool call block from `assistantMessage.content`.
	pub tool_call: AgentToolCall,
	/// Validated tool arguments for the target tool schema.
	pub args: Value,
	/// The executed tool result before any `afterToolCall` overrides are applied.
	pub result: AgentToolResult,
	/// Whether the executed tool result is currently treated as an error.
	pub is_error: bool,
	/// Current agent context at the time the tool call is finalized.
	pub context: AgentContext,
}

/// Context passed to completed-turn callbacks.
#[derive(Clone, Debug)]
pub struct AgentTurnContext {
	/// The assistant message that completed the turn.
	pub message: AssistantMessage,
	/// Tool result messages emitted for the completed turn.
	pub tool_results: Vec<ToolResultMessage>,
	/// Current agent context after the turn's assistant message and tool results have been appended.
	pub context: AgentContext,
	/// Messages that this loop invocation will return if it exits at this point. Prompt runs include the initial prompt messages; continuation runs do not include pre-existing context messages.
	pub new_messages: Vec<AgentMessage>,
}

/// Decision returned by [`FinishTurn`]. Returning undefined preserves normal scheduling.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "action")]
pub enum AgentTurnDecision {
	#[serde(rename = "continue")]
	Continue,
	#[serde(rename = "end")]
	End,
}

/// Called after a completed assistant turn and all of its tool-result messages, but before `turn_end`.
/// On a normal turn, `{ action: "continue" }` ensures one next provider request. Tool-result, steering, or
/// follow-up scheduling can satisfy that request and adds no extra request; otherwise the loop continues once
/// with the current context. Error and aborted responses remain hard exits.
pub type FinishTurn =
	Rc<dyn Fn(AgentTurnContext, Option<AbortSignal>) -> LocalBoxFuture<'static, PiResult<Option<AgentTurnDecision>>>>;

/// Replacement runtime state used by the agent loop before starting another provider request.
#[derive(Clone, Debug, Default)]
pub struct AgentLoopTurnUpdate {
	/// Context for the next provider request.
	pub context: Option<AgentContext>,
	/// Messages to append before the next provider request, with normal lifecycle events.
	pub messages: Option<Vec<AgentMessage>>,
	/// Model for the next provider request.
	pub model: Option<Model>,
	/// Thinking level for the next provider request.
	pub thinking_level: Option<ThinkingLevel>,
}

/// Runtime state available immediately before a conversational provider request.
#[derive(Clone, Debug)]
pub struct PrepareRequestContext {
	pub context: AgentContext,
	pub model: Model,
	pub thinking_level: ThinkingLevel,
}

/// Replacement runtime state for the provider request being prepared (`Omit<AgentLoopTurnUpdate, "messages">`).
#[derive(Clone, Debug, Default)]
pub struct AgentRequestUpdate {
	/// Context for the next provider request.
	pub context: Option<AgentContext>,
	/// Model for the next provider request.
	pub model: Option<Model>,
	/// Thinking level for the next provider request.
	pub thinking_level: Option<ThinkingLevel>,
}

/// Called immediately before every conversational provider request, including the first.
/// Pending messages have already been appended and emitted when this callback runs.
pub type PrepareRequest = Rc<
	dyn Fn(PrepareRequestContext, Option<AbortSignal>) -> LocalBoxFuture<'static, PiResult<Option<AgentRequestUpdate>>>,
>;

/// `PrepareNextTurnContext extends AgentTurnContext {}`.
pub type PrepareNextTurnContext = AgentTurnContext;

/// `AgentLoopConfig.convertToLlm`.
pub type ConvertToLlmFn = Rc<dyn Fn(Vec<AgentMessage>) -> LocalBoxFuture<'static, PiResult<Vec<Message>>>>;
/// `AgentLoopConfig.transformContext`.
pub type TransformContextFn =
	Rc<dyn Fn(Vec<AgentMessage>, Option<AbortSignal>) -> LocalBoxFuture<'static, PiResult<Vec<AgentMessage>>>>;
/// `AgentLoopConfig.getApiKey`.
pub type GetApiKeyFn = Rc<dyn Fn(String) -> LocalBoxFuture<'static, PiResult<Option<String>>>>;
/// `AgentLoopConfig.prepareNextTurn`.
pub type PrepareNextTurnFn =
	Rc<dyn Fn(PrepareNextTurnContext) -> LocalBoxFuture<'static, PiResult<Option<AgentLoopTurnUpdate>>>>;
/// `AgentLoopConfig.getSteeringMessages` and `getFollowUpMessages`.
pub type GetMessagesFn = Rc<dyn Fn() -> LocalBoxFuture<'static, PiResult<Vec<AgentMessage>>>>;
/// `AgentLoopConfig.beforeToolCall`.
pub type BeforeToolCallFn = Rc<
	dyn Fn(BeforeToolCallContext, Option<AbortSignal>) -> LocalBoxFuture<'static, PiResult<Option<BeforeToolCallResult>>>,
>;
/// `AgentLoopConfig.afterToolCall`.
pub type AfterToolCallFn = Rc<
	dyn Fn(AfterToolCallContext, Option<AbortSignal>) -> LocalBoxFuture<'static, PiResult<Option<AfterToolCallResult>>>,
>;

/// `AgentLoopConfig extends SimpleStreamOptions`: the stream options are `base`.
#[derive(Clone)]
pub struct AgentLoopConfig {
	pub base: SimpleStreamOptions,
	pub model: Model,

	/// Converts AgentMessage[] to LLM-compatible Message[] before each LLM call.
	///
	/// Each AgentMessage must be converted to a SystemMessage, UserMessage, AssistantMessage, or ToolResultMessage
	/// that the LLM can understand. AgentMessages that cannot be converted (e.g., UI-only notifications,
	/// status messages) should be filtered out.
	///
	/// Contract: must not throw or reject. Return a safe fallback value instead.
	/// Throwing interrupts the low-level agent loop without producing a normal event sequence.
	pub convert_to_llm: ConvertToLlmFn,

	/// Optional transform applied to the context before `convertToLlm`.
	///
	/// Use this for operations that work at the AgentMessage level:
	/// - Context window management (pruning old messages)
	/// - Injecting context from external sources
	///
	/// Contract: must not throw or reject. Return the original messages or another
	/// safe fallback value instead.
	pub transform_context: Option<TransformContextFn>,

	/// Resolves an API key dynamically for each LLM call.
	///
	/// Useful for short-lived OAuth tokens (e.g., GitHub Copilot) that may expire
	/// during long-running tool execution phases.
	///
	/// Contract: must not throw or reject. Return undefined when no key is available.
	pub get_api_key: Option<GetApiKeyFn>,

	/// Called after the assistant message and all tool-result messages have been emitted, immediately before `turn_end`.
	/// `{ action: "end" }` ends the run without polling queues or preparing another request.
	/// On a normal turn, `{ action: "continue" }` ensures one next provider request. Tool-result, steering, or
	/// follow-up scheduling can satisfy that request and adds no extra request; otherwise the loop continues once
	/// with the current context. Returning undefined preserves normal scheduling. Error and aborted responses remain
	/// hard exits.
	pub finish_turn: Option<FinishTurn>,

	/// Called immediately before every conversational provider request, including the first.
	/// Pending messages have already been appended. The returned context, model, and thinking level
	/// replace the runtime values for this and later requests in the run. This hook does not poll queues.
	pub prepare_request: Option<PrepareRequest>,

	/// Called after `turn_end` when the loop will continue, immediately before the next turn starts.
	/// Return replacement context/model/thinking state or messages to append to affect that turn.
	/// Return undefined to keep using the current context/config.
	pub prepare_next_turn: Option<PrepareNextTurnFn>,

	/// Returns steering messages to inject into the conversation mid-run.
	///
	/// Called after the current assistant turn finishes executing its tool calls, unless `finishTurn` ends the run.
	/// If messages are returned, they are added to the context before the next LLM call.
	/// Tool calls from the current assistant message are not skipped.
	///
	/// Use this for "steering" the agent while it's working.
	///
	/// Contract: must not throw or reject. Return [] when no steering messages are available.
	pub get_steering_messages: Option<GetMessagesFn>,

	/// Returns follow-up messages to process after the agent would otherwise stop.
	///
	/// Called when the agent has no more tool calls and no steering messages.
	/// If messages are returned, they're added to the context and the agent
	/// continues with another turn.
	///
	/// Use this for follow-up messages that should wait until the agent finishes.
	///
	/// Contract: must not throw or reject. Return [] when no follow-up messages are available.
	pub get_follow_up_messages: Option<GetMessagesFn>,

	/// Tool execution mode.
	/// - "sequential": execute tool calls one by one
	/// - "parallel": preflight tool calls sequentially, then execute allowed tools concurrently;
	///   emit `tool_execution_end` in tool completion order after each tool is finalized,
	///   then emit tool-result message artifacts later in assistant source order
	///
	/// Default: "parallel"
	pub tool_execution: Option<ToolExecutionMode>,

	/// Called before a tool is executed, after arguments have been validated.
	///
	/// Return `{ block: true }` to prevent execution. The loop emits an error tool result instead.
	/// A blocked result can also set `terminate: true` to participate in the batch early-termination rule.
	/// The hook receives the agent abort signal and is responsible for honoring it.
	pub before_tool_call: Option<BeforeToolCallFn>,

	/// Called after a tool finishes executing, before `tool_execution_end` and tool-result message events are emitted.
	///
	/// Return an `AfterToolCallResult` to override parts of the executed tool result:
	/// - `content` replaces the full content array
	/// - `details` replaces the full details payload
	/// - `isError` replaces the error flag
	/// - `usage` replaces the tool result usage
	/// - `terminate` replaces the early-termination hint
	///
	/// Any omitted fields keep their original values. No deep merge is performed.
	/// The hook receives the agent abort signal and is responsible for honoring it.
	pub after_tool_call: Option<AfterToolCallFn>,
}

impl AgentLoopConfig {
	/// A config with the two required fields and every optional field unset.
	pub fn new(model: Model, convert_to_llm: ConvertToLlmFn) -> Self {
		Self {
			base: SimpleStreamOptions::default(),
			model,
			convert_to_llm,
			transform_context: None,
			get_api_key: None,
			finish_turn: None,
			prepare_request: None,
			prepare_next_turn: None,
			get_steering_messages: None,
			get_follow_up_messages: None,
			tool_execution: None,
			before_tool_call: None,
			after_tool_call: None,
		}
	}
}

impl Deref for AgentLoopConfig {
	type Target = SimpleStreamOptions;

	fn deref(&self) -> &Self::Target {
		&self.base
	}
}

impl DerefMut for AgentLoopConfig {
	fn deref_mut(&mut self) -> &mut Self::Target {
		&mut self.base
	}
}

/// Thinking/reasoning level for models that support it.
/// Note: "xhigh" and "max" are only supported by selected model families. Use model
/// thinking-level metadata from @at-inc/pi-ai to detect support for a concrete model.
pub type ThinkingLevel = ModelThinkingLevel;

/// AgentMessage: Union of LLM messages + custom messages.
/// This abstraction allows apps to add custom message types while maintaining
/// type safety and compatibility with the base LLM messages.
///
/// Deserializes as `Llm` for the roles `system`, `user`, `assistant` and `toolResult`, and as `Custom` otherwise;
/// serializes as the message itself.
#[derive(Clone, Debug, PartialEq)]
pub enum AgentMessage {
	Llm(Message),
	Custom(Value),
}

const LLM_ROLES: [&str; 4] = ["system", "user", "assistant", "toolResult"];

impl AgentMessage {
	/// `message.role`. A custom message without a string `role` reads as `""`.
	pub fn role(&self) -> &str {
		match self {
			AgentMessage::Llm(Message::System(_)) => "system",
			AgentMessage::Llm(Message::User(_)) => "user",
			AgentMessage::Llm(Message::Assistant(_)) => "assistant",
			AgentMessage::Llm(Message::ToolResult(_)) => "toolResult",
			AgentMessage::Custom(value) => value.get("role").and_then(Value::as_str).unwrap_or(""),
		}
	}

	/// The LLM message, when this is one.
	pub fn as_llm(&self) -> Option<&Message> {
		match self {
			AgentMessage::Llm(message) => Some(message),
			AgentMessage::Custom(_) => None,
		}
	}
}

impl TranscriptMessage for AgentMessage {
	fn as_system_message(&self) -> Option<&SystemMessage> {
		match self {
			AgentMessage::Llm(Message::System(message)) => Some(message),
			_ => None,
		}
	}
}

impl From<Message> for AgentMessage {
	fn from(message: Message) -> Self {
		AgentMessage::Llm(message)
	}
}

impl From<SystemMessage> for AgentMessage {
	fn from(message: SystemMessage) -> Self {
		AgentMessage::Llm(Message::System(message))
	}
}

impl From<UserMessage> for AgentMessage {
	fn from(message: UserMessage) -> Self {
		AgentMessage::Llm(Message::User(message))
	}
}

impl From<AssistantMessage> for AgentMessage {
	fn from(message: AssistantMessage) -> Self {
		AgentMessage::Llm(Message::Assistant(message))
	}
}

impl From<ToolResultMessage> for AgentMessage {
	fn from(message: ToolResultMessage) -> Self {
		AgentMessage::Llm(Message::ToolResult(message))
	}
}

impl Serialize for AgentMessage {
	fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
		match self {
			AgentMessage::Llm(message) => message.serialize(serializer),
			AgentMessage::Custom(value) => value.serialize(serializer),
		}
	}
}

impl<'de> Deserialize<'de> for AgentMessage {
	fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
		let value = Value::deserialize(deserializer)?;
		let is_llm = value
			.get("role")
			.and_then(Value::as_str)
			.is_some_and(|role| LLM_ROLES.contains(&role));
		if !is_llm {
			return Ok(AgentMessage::Custom(value));
		}
		serde_json::from_value(value)
			.map(AgentMessage::Llm)
			.map_err(serde::de::Error::custom)
	}
}

/// What `AgentState.streamingMessage` holds: a message, or the live partial of the response being streamed.
#[derive(Clone, Debug)]
pub(crate) enum StreamingMessage {
	Message(AgentMessage),
	Partial(SharedAssistantMessage),
}

/// Public agent state.
///
/// `tools` and `messages` are accessor methods; assigning a new list replaces the stored one.
#[derive(Clone, Debug)]
pub struct AgentState {
	/// Active model used for future turns.
	pub model: Model,
	/// Requested reasoning level for future turns.
	pub thinking_level: ThinkingLevel,
	pub(crate) tools: Vec<AgentTool>,
	pub(crate) messages: Vec<AgentMessage>,
	pub(crate) is_streaming: bool,
	pub(crate) streaming_message: Option<StreamingMessage>,
	pub(crate) pending_tool_calls: IndexSet<String>,
	pub(crate) error_message: Option<String>,
}

impl AgentState {
	/// Current system prompt, replayed from the transcript's system messages.
	///
	/// Read-only: to change the prompt, append a system message with `content` or `sections`.
	/// In `initialState`, this seeds the leading system message.
	pub fn system_prompt(&self) -> String {
		pi_ai::utils::transcript::get_current_system_prompt(&self.messages)
	}

	/// Executable tools.
	///
	/// Differences from the tools declared in the transcript are announced to the model
	/// with a system message before the next request.
	pub fn tools(&self) -> &[AgentTool] {
		&self.tools
	}

	/// The tool list, for in-place changes.
	pub fn tools_mut(&mut self) -> &mut Vec<AgentTool> {
		&mut self.tools
	}

	/// `state.tools = tools`.
	pub fn set_tools(&mut self, tools: Vec<AgentTool>) {
		self.tools = tools;
	}

	/// Conversation transcript.
	///
	/// System messages in the transcript carry the prompt and tool declarations.
	pub fn messages(&self) -> &[AgentMessage] {
		&self.messages
	}

	/// The transcript, for in-place changes (`state.messages.push(...)`).
	pub fn messages_mut(&mut self) -> &mut Vec<AgentMessage> {
		&mut self.messages
	}

	/// `state.messages = messages`.
	pub fn set_messages(&mut self, messages: Vec<AgentMessage>) {
		self.messages = messages;
	}

	/// True while the agent is processing a prompt or continuation.
	///
	/// This remains true until awaited `agent_end` listeners settle.
	pub fn is_streaming(&self) -> bool {
		self.is_streaming
	}

	/// Partial assistant message for the current streamed response, if any.
	pub fn streaming_message(&self) -> Option<AgentMessage> {
		match self.streaming_message.as_ref()? {
			StreamingMessage::Message(message) => Some(message.clone()),
			StreamingMessage::Partial(partial) => Some(AgentMessage::Llm(Message::Assistant(partial.borrow().clone()))),
		}
	}

	/// Tool call ids currently executing.
	pub fn pending_tool_calls(&self) -> &IndexSet<String> {
		&self.pending_tool_calls
	}

	/// Error message from the most recent failed or aborted assistant turn, if any.
	pub fn error_message(&self) -> Option<&str> {
		self.error_message.as_deref()
	}
}

/// Final or partial result produced by a tool.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentToolResult {
	/// Text or image content returned to the model. A `null` from an untyped tool reads as empty.
	#[serde(default, deserialize_with = "null_as_empty")]
	pub content: Vec<UserContent>,
	/// Arbitrary structured details for logs or UI rendering. `None` is `undefined`; an explicit `null` stays
	/// `Some(Value::Null)`.
	#[serde(default, deserialize_with = "present_json_value", skip_serializing_if = "Option::is_none")]
	pub details: Option<Value>,
	/// Machine-readable result matching the tool's `outputSchema`, for programmatic callers. Not sent
	/// to the model; `content` remains the model-facing result.
	#[serde(default, deserialize_with = "present_json_value", skip_serializing_if = "Option::is_none")]
	pub structured_content: Option<Value>,
	/// Usage from the final tool execution itself, if available. Not used for main LLM context accounting.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub usage: Option<Usage>,
	/// Report a failure without throwing. The model sees `content` as an error result, like a thrown
	/// error, but `details` and `structuredContent` are kept for the UI and programmatic callers.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub is_error: Option<bool>,
	/// Hint that the agent should stop after the current tool batch.
	/// Early termination only happens when every finalized tool result in the batch sets this to true.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub terminate: Option<bool>,
	/// App fields the TS object carried beyond this interface; kept so they survive a round trip.
	#[serde(flatten)]
	pub extra: Map<String, Value>,
}

fn null_as_empty<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Vec<UserContent>, D::Error> {
	Ok(Option::<Vec<UserContent>>::deserialize(deserializer)?.unwrap_or_default())
}

fn present_json_value<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Option<Value>, D::Error> {
	Value::deserialize(deserializer).map(Some)
}

/// Final outcome of a tool call after hooks ran.
#[derive(Clone, Debug, PartialEq)]
pub struct AgentToolCallOutcome {
	pub tool_call: AgentToolCall,
	pub result: AgentToolResult,
	pub is_error: bool,
	/// Milliseconds `execute()` took, measured with a monotonic clock; absent when the tool did not run.
	pub duration_ms: Option<f64>,
}

/// Callback used by tools to stream partial execution updates.
///
/// The callback is scoped to the current `execute()` invocation. Calls made after
/// the tool promise settles are ignored.
pub type AgentToolUpdateCallback = Rc<dyn Fn(AgentToolResult)>;

/// `AgentTool.execute`: `(toolCallId, params, signal, onUpdate)`.
pub type AgentToolExecute = Rc<
	dyn Fn(
		String,
		Value,
		Option<AbortSignal>,
		Option<AgentToolUpdateCallback>,
	) -> LocalBoxFuture<'static, PiResult<AgentToolResult>>,
>;

/// `AgentTool.prepareArguments`. Its documented contract (return an object matching the parameters) is part of
/// the type.
pub type PrepareArgumentsFn = Rc<dyn Fn(Value) -> PiResult<Map<String, Value>>>;

/// `AgentTool.replay`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ToolReplay {
	#[serde(rename = "never")]
	Never,
	#[serde(rename = "safe")]
	Safe,
}

/// Tool definition used by the agent runtime. `parameters` is any JSON Schema object. `AgentTool extends Tool`: the
/// declaration fields are `tool`.
#[derive(Clone)]
pub struct AgentTool {
	pub tool: Tool,
	/// Human-readable label for UI display.
	pub label: String,
	/// Optional compatibility shim for raw tool-call arguments before schema validation.
	/// Must return an object that matches `TParameters`.
	pub prepare_arguments: Option<PrepareArgumentsFn>,
	/// JSON Schema of `structuredContent` in successful results. Tools that declare it should always
	/// set `structuredContent`.
	pub output_schema: Option<Value>,
	/// Execute the tool call. Throw on failure, or return a result with `isError: true`; do not only
	/// describe the failure in `content`.
	pub execute: AgentToolExecute,
	/// Recovery policy for an effect whose durable intent exists but whose outcome is unknown.
	pub replay: Option<ToolReplay>,
	/// Per-tool execution mode override.
	/// - "sequential": this tool must execute one at a time with other tool calls.
	/// - "parallel": this tool can execute concurrently with other tool calls.
	///
	/// If omitted, the default execution mode applies.
	pub execution_mode: Option<ToolExecutionMode>,
}

impl Deref for AgentTool {
	type Target = Tool;

	fn deref(&self) -> &Self::Target {
		&self.tool
	}
}

impl DerefMut for AgentTool {
	fn deref_mut(&mut self) -> &mut Self::Target {
		&mut self.tool
	}
}

impl std::fmt::Debug for AgentTool {
	fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
		f.debug_struct("AgentTool")
			.field("tool", &self.tool)
			.field("label", &self.label)
			.field("prepare_arguments", &self.prepare_arguments.is_some())
			.field("output_schema", &self.output_schema)
			.field("replay", &self.replay)
			.field("execution_mode", &self.execution_mode)
			.finish_non_exhaustive()
	}
}

/// Context snapshot passed into the low-level agent loop.
#[derive(Clone, Debug, Default)]
pub struct AgentContext {
	/// Transcript visible to the model.
	pub messages: Vec<AgentMessage>,
	/// Tools available for execution in this run.
	pub tools: Option<Vec<AgentTool>>,
}

/// Events emitted by the Agent for UI updates.
///
/// `agent_end` is the last event emitted for a run, but awaited `Agent.subscribe()`
/// listeners for that event are still part of run settlement. The agent becomes
/// idle only after those listeners finish.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", rename_all_fields = "camelCase")]
pub enum AgentEvent {
	// Agent lifecycle
	AgentStart,
	AgentEnd {
		messages: Vec<AgentMessage>,
	},
	// Turn lifecycle - a turn is one assistant response + any tool calls/results
	TurnStart,
	TurnEnd {
		message: AgentMessage,
		tool_results: Vec<ToolResultMessage>,
	},
	// Message lifecycle - emitted for system, user, assistant, and toolResult messages
	MessageStart {
		message: AgentMessage,
	},
	/// Only emitted for assistant messages during streaming. `message` is the live partial of the response.
	MessageUpdate {
		message: SharedAssistantMessage,
		assistant_message_event: AssistantMessageEvent,
	},
	MessageEnd {
		message: AgentMessage,
	},
	// Tool execution lifecycle
	ToolExecutionStart {
		tool_call_id: String,
		tool_name: String,
		args: Value,
	},
	ToolExecutionUpdate {
		tool_call_id: String,
		tool_name: String,
		args: Value,
		partial_result: AgentToolResult,
	},
	ToolExecutionEnd {
		tool_call_id: String,
		tool_name: String,
		result: AgentToolResult,
		is_error: bool,
		/// Milliseconds `execute()` took, measured with a monotonic clock; absent when the tool did not run.
		#[serde(default, skip_serializing_if = "Option::is_none")]
		duration_ms: Option<f64>,
	},
}
