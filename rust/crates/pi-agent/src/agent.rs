//! Port of `packages/agent/src/agent.ts`.
//! PORT-STATUS: reviewed
//!
//! Rust differences:
//! - `Agent` is a cheap handle (`Rc`); clones share one agent, like references to the TS object.
//! - The public mutable properties of the class (`convertToLlm`, `streamFunction`, the hooks, `sessionId`, ...)
//!   are the fields of [`AgentProperties`], read and written through `properties()` / `properties_mut()`. `state` is
//!   `state()` / `state_mut()`.
//! - `prompt(input, images?)` takes a [`PromptInput`]; `continue()` is `r#continue()`.
//! - The constructor throws when no stream function is configured, so `Agent::new` returns a `PiResult`.
//! - A listener is compared by `Rc` pointer, like the `Set` of functions: subscribing the same `Rc` twice keeps one
//!   entry; two `Rc`s around the same function are two listeners.
//! - Each listener receives its own copy of the event, so a listener that edits `event.message` changes neither the
//!   transcript entry nor what later listeners see. In TS they all share one object.
//! - `defaultConvertToLlm` keeps `AgentMessage::Llm` only. A hand-built `AgentMessage::Custom` with an LLM role is
//!   dropped (TS keeps it by role), and `turn_end` reads `errorMessage` only from an `Llm` assistant message.
//!   Deserialization never builds such a message: the four LLM roles always become `Llm`.
//! - Holding `state_mut()` or `properties_mut()` across `prompt()` / `continue()` or an await of the run panics with a
//!   `RefCell` borrow error; TS has no such restriction.
//! - `prompt()` / `continue()` start the run when called and return its future; the loop itself (`agent_start` and
//!   the first listener call) runs once the future is polled, where TS runs it up to the first `await` inside the
//!   call. Dropping the future ends the run the way `finally` does (`finishRun`); a TS promise cannot be dropped.
//! - `state.streamingMessage` during `message_update` is the provider's live partial (see `types.rs`), so its
//!   top-level fields read the provider's latest values, where TS keeps the shallow copy taken at that update.

use std::cell::{Cell, Ref, RefCell, RefMut};
use std::rc::Rc;
use std::sync::LazyLock;

use futures::FutureExt;
use futures::channel::oneshot;
use futures::future::Shared;
use indexmap::IndexSet;
use pi_ai::types::{
	AssistantContent, AssistantMessage, ImageContent, Message, Model, OnPayload, OnProviderStreamEvent, OnResponse,
	StopReason, TextContent, ThinkingBudgets, Transport, Usage, UserContent, UserMessage, UserMessageContent,
};
use pi_ai::utils::identity::ClientIdentity;
use pi_ai::utils::transcript::{create_initial_system_message, get_current_system_message, to_tool_declaration};
use pi_host::{AbortController, AbortSignal, LocalBoxFuture, PiError, PiResult};

use crate::agent_loop::{AgentEventSink, reasoning_for, run_agent_loop, run_agent_loop_continue};
use crate::stream_fn::get_default_stream_fn;
use crate::types::{
	AfterToolCallFn, AgentContext, AgentEvent, AgentLoopConfig, AgentLoopTurnUpdate, AgentMessage, AgentState,
	AgentTool, BeforeToolCallFn, ConvertToLlmFn, FinishTurn, GetApiKeyFn, PrepareNextTurnContext, PrepareRequest,
	QueueMode, StreamFn, StreamingMessage, ThinkingLevel, ToolExecutionMode, TransformContextFn,
};

fn default_convert_to_llm(messages: Vec<AgentMessage>) -> LocalBoxFuture<'static, PiResult<Vec<Message>>> {
	let messages = messages
		.into_iter()
		.filter_map(|message| match message {
			AgentMessage::Llm(message) => Some(message),
			AgentMessage::Custom(_) => None,
		})
		.collect();
	Box::pin(async move { Ok(messages) })
}

static DEFAULT_MODEL: LazyLock<serde_json::Value> = LazyLock::new(|| {
	serde_json::json!({
		"id": "unknown",
		"name": "unknown",
		"api": "unknown",
		"provider": "unknown",
		"baseUrl": "",
		"reasoning": false,
		"input": [],
		"cost": { "input": 0, "output": 0, "cacheRead": 0, "cacheWrite": 0 },
		"contextWindow": 0,
		"maxTokens": 0,
	})
});

fn default_model() -> Model {
	serde_json::from_value(DEFAULT_MODEL.clone()).expect("DEFAULT_MODEL satisfies Model")
}

/// Initial state for [`Agent`]. `systemPrompt` and `tools` become the leading system message unless `messages`
/// already starts with one.
#[derive(Clone, Debug, Default)]
pub struct AgentInitialState {
	pub system_prompt: Option<String>,
	pub model: Option<Model>,
	pub thinking_level: Option<ThinkingLevel>,
	pub tools: Option<Vec<AgentTool>>,
	pub messages: Option<Vec<AgentMessage>>,
}

fn create_mutable_agent_state(initial_state: AgentInitialState) -> AgentState {
	let tools = initial_state.tools.unwrap_or_default();
	let mut messages = initial_state.messages.unwrap_or_default();
	let declarations: Vec<_> = tools.iter().map(|tool| to_tool_declaration(&tool.tool)).collect();
	let initial_message = create_initial_system_message(initial_state.system_prompt.as_deref(), Some(&declarations));
	if let Some(initial_message) = initial_message
		&& messages.first().map(AgentMessage::role) != Some("system")
	{
		messages.insert(0, initial_message.into());
	}

	AgentState {
		model: initial_state.model.unwrap_or_else(default_model),
		thinking_level: initial_state.thinking_level.unwrap_or(ThinkingLevel::Off),
		tools,
		messages,
		is_streaming: false,
		streaming_message: None,
		pending_tool_calls: IndexSet::new(),
		error_message: None,
	}
}

/// `AgentOptions.prepareNextTurn`.
pub type AgentPrepareNextTurnFn =
	Rc<dyn Fn(Option<AbortSignal>) -> LocalBoxFuture<'static, PiResult<Option<AgentLoopTurnUpdate>>>>;
/// `AgentOptions.prepareNextTurnWithContext`.
pub type AgentPrepareNextTurnWithContextFn = Rc<
	dyn Fn(
		PrepareNextTurnContext,
		Option<AbortSignal>,
	) -> LocalBoxFuture<'static, PiResult<Option<AgentLoopTurnUpdate>>>,
>;
/// A listener passed to [`Agent::subscribe`].
pub type AgentListener = Rc<dyn Fn(AgentEvent, AbortSignal) -> LocalBoxFuture<'static, PiResult<()>>>;

/// Options for constructing an [`Agent`].
#[derive(Clone, Default)]
pub struct AgentOptions {
	pub initial_state: Option<AgentInitialState>,
	pub convert_to_llm: Option<ConvertToLlmFn>,
	pub transform_context: Option<TransformContextFn>,
	pub stream_fn: Option<StreamFn>,
	pub get_api_key: Option<GetApiKeyFn>,
	pub on_payload: Option<OnPayload>,
	pub on_response: Option<OnResponse>,
	pub on_provider_stream_event: Option<OnProviderStreamEvent>,
	pub before_tool_call: Option<BeforeToolCallFn>,
	pub after_tool_call: Option<AfterToolCallFn>,
	pub finish_turn: Option<FinishTurn>,
	pub prepare_request: Option<PrepareRequest>,
	pub prepare_next_turn: Option<AgentPrepareNextTurnFn>,
	pub prepare_next_turn_with_context: Option<AgentPrepareNextTurnWithContextFn>,
	pub steering_mode: Option<QueueMode>,
	pub follow_up_mode: Option<QueueMode>,
	pub session_id: Option<String>,
	pub thinking_budgets: Option<ThinkingBudgets>,
	pub transport: Option<Transport>,
	pub max_retry_delay_ms: Option<f64>,
	pub tool_execution: Option<ToolExecutionMode>,
	/// How requests identify the app to providers, merged field by field over `DEFAULT_AGENT_IDENTITY`
	/// (`{ attribution: PI_ATTRIBUTION }`). Pass `{ attribution: null }` to send no attribution headers.
	pub identity: Option<ClientIdentity>,
}

struct PendingMessageQueue {
	messages: Vec<AgentMessage>,
	mode: QueueMode,
}

impl PendingMessageQueue {
	fn new(mode: QueueMode) -> Self {
		Self {
			messages: Vec::new(),
			mode,
		}
	}

	fn enqueue(&mut self, message: AgentMessage) {
		self.messages.push(message);
	}

	fn has_items(&self) -> bool {
		!self.messages.is_empty()
	}

	fn peek(&self) -> Vec<AgentMessage> {
		if self.mode == QueueMode::All {
			return self.messages.clone();
		}
		self.messages.first().cloned().into_iter().collect()
	}

	fn drain(&mut self) -> Vec<AgentMessage> {
		let drained = self.peek();
		self.messages.drain(..drained.len());
		drained
	}

	fn clear(&mut self) {
		self.messages.clear();
	}
}

struct ActiveRun {
	promise: Shared<LocalBoxFuture<'static, ()>>,
	resolve: oneshot::Sender<()>,
	abort_controller: AbortController,
}

/// The public mutable properties of the TS `Agent` class.
#[derive(Clone)]
pub struct AgentProperties {
	pub convert_to_llm: ConvertToLlmFn,
	pub transform_context: Option<TransformContextFn>,
	pub stream_function: StreamFn,
	pub get_api_key: Option<GetApiKeyFn>,
	pub on_payload: Option<OnPayload>,
	pub on_response: Option<OnResponse>,
	pub on_provider_stream_event: Option<OnProviderStreamEvent>,
	pub before_tool_call: Option<BeforeToolCallFn>,
	pub after_tool_call: Option<AfterToolCallFn>,
	pub finish_turn: Option<FinishTurn>,
	pub prepare_request: Option<PrepareRequest>,
	pub prepare_next_turn: Option<AgentPrepareNextTurnFn>,
	pub prepare_next_turn_with_context: Option<AgentPrepareNextTurnWithContextFn>,
	/// Session identifier forwarded to providers for cache-aware backends.
	pub session_id: Option<String>,
	/// Optional per-level thinking token budgets forwarded to the stream function.
	pub thinking_budgets: Option<ThinkingBudgets>,
	/// Preferred transport forwarded to the stream function.
	pub transport: Transport,
	/// Optional cap for provider-requested retry delays.
	pub max_retry_delay_ms: Option<f64>,
	/// Tool execution strategy for assistant messages that contain multiple tool calls.
	pub tool_execution: ToolExecutionMode,
	/// Request identity forwarded to the stream function, over `DEFAULT_AGENT_IDENTITY`.
	pub identity: Option<ClientIdentity>,
}

struct AgentInner {
	state: RefCell<AgentState>,
	listeners: RefCell<Vec<(u64, AgentListener)>>,
	next_listener_id: Cell<u64>,
	steering_queue: RefCell<PendingMessageQueue>,
	follow_up_queue: RefCell<PendingMessageQueue>,
	properties: RefCell<AgentProperties>,
	active_run: RefCell<Option<ActiveRun>>,
}

/// `prompt()` input: text with optional images, a single message, or a batch of messages.
#[derive(Clone, Debug)]
pub enum PromptInput {
	Text {
		text: String,
		images: Option<Vec<ImageContent>>,
	},
	Message(AgentMessage),
	Messages(Vec<AgentMessage>),
}

impl From<&str> for PromptInput {
	fn from(text: &str) -> Self {
		PromptInput::Text {
			text: text.to_string(),
			images: None,
		}
	}
}

impl From<String> for PromptInput {
	fn from(text: String) -> Self {
		PromptInput::Text { text, images: None }
	}
}

impl From<AgentMessage> for PromptInput {
	fn from(message: AgentMessage) -> Self {
		PromptInput::Message(message)
	}
}

impl From<UserMessage> for PromptInput {
	fn from(message: UserMessage) -> Self {
		PromptInput::Message(message.into())
	}
}

impl From<Vec<AgentMessage>> for PromptInput {
	fn from(messages: Vec<AgentMessage>) -> Self {
		PromptInput::Messages(messages)
	}
}

/// Stateful wrapper around the low-level agent loop.
///
/// `Agent` owns the current transcript, emits lifecycle events, executes tools,
/// and exposes queueing APIs for steering and follow-up messages.
#[derive(Clone)]
pub struct Agent {
	inner: Rc<AgentInner>,
}

impl Agent {
	/// `new Agent(options)`. Fails when `options.stream_fn` is unset and no default stream function is configured.
	pub fn new(options: AgentOptions) -> PiResult<Self> {
		let stream_function = match options.stream_fn {
			Some(stream_fn) => stream_fn,
			None => get_default_stream_fn()?,
		};
		let convert_to_llm: ConvertToLlmFn = options
			.convert_to_llm
			.unwrap_or_else(|| Rc::new(default_convert_to_llm));
		Ok(Self {
			inner: Rc::new(AgentInner {
				state: RefCell::new(create_mutable_agent_state(options.initial_state.unwrap_or_default())),
				listeners: RefCell::new(Vec::new()),
				next_listener_id: Cell::new(0),
				steering_queue: RefCell::new(PendingMessageQueue::new(
					options.steering_mode.unwrap_or(QueueMode::OneAtATime),
				)),
				follow_up_queue: RefCell::new(PendingMessageQueue::new(
					options.follow_up_mode.unwrap_or(QueueMode::OneAtATime),
				)),
				properties: RefCell::new(AgentProperties {
					convert_to_llm,
					transform_context: options.transform_context,
					stream_function,
					get_api_key: options.get_api_key,
					on_payload: options.on_payload,
					on_response: options.on_response,
					on_provider_stream_event: options.on_provider_stream_event,
					before_tool_call: options.before_tool_call,
					after_tool_call: options.after_tool_call,
					finish_turn: options.finish_turn,
					prepare_request: options.prepare_request,
					prepare_next_turn: options.prepare_next_turn,
					prepare_next_turn_with_context: options.prepare_next_turn_with_context,
					session_id: options.session_id,
					thinking_budgets: options.thinking_budgets,
					transport: options.transport.unwrap_or(Transport::Auto),
					max_retry_delay_ms: options.max_retry_delay_ms,
					tool_execution: options.tool_execution.unwrap_or(ToolExecutionMode::Parallel),
					identity: options.identity,
				}),
				active_run: RefCell::new(None),
			}),
		})
	}

	/// The public properties (`agent.streamFunction`, `agent.beforeToolCall`, ...).
	pub fn properties(&self) -> Ref<'_, AgentProperties> {
		self.inner.properties.borrow()
	}

	/// The public properties, for assignment (`agent.sessionId = ...`).
	pub fn properties_mut(&self) -> RefMut<'_, AgentProperties> {
		self.inner.properties.borrow_mut()
	}

	/// Subscribe to agent lifecycle events.
	///
	/// Listener promises are awaited in subscription order and are included in
	/// the current run's settlement. Listeners also receive the active abort
	/// signal for the current run.
	///
	/// `agent_end` is the final emitted event for a run, but the agent does not
	/// become idle until all awaited listeners for that event have settled.
	///
	/// Returns the unsubscribe function, which reports whether the listener was still subscribed.
	pub fn subscribe(&self, listener: AgentListener) -> Box<dyn Fn() -> bool> {
		{
			let mut listeners = self.inner.listeners.borrow_mut();
			if !listeners.iter().any(|(_, existing)| same_listener(existing, &listener)) {
				let id = self.inner.next_listener_id.get();
				self.inner.next_listener_id.set(id + 1);
				listeners.push((id, listener.clone()));
			}
		}
		let inner = Rc::downgrade(&self.inner);
		Box::new(move || {
			let Some(inner) = inner.upgrade() else {
				return false;
			};
			let mut listeners = inner.listeners.borrow_mut();
			let before = listeners.len();
			listeners.retain(|(_, existing)| !same_listener(existing, &listener));
			listeners.len() != before
		})
	}

	/// Current agent state.
	///
	/// Assigning `state.tools` or `state.messages` copies the provided top-level array.
	pub fn state(&self) -> Ref<'_, AgentState> {
		self.inner.state.borrow()
	}

	/// Current agent state, for assignment (`agent.state.model = ...`).
	pub fn state_mut(&self) -> RefMut<'_, AgentState> {
		self.inner.state.borrow_mut()
	}

	/// Controls how queued steering messages are drained.
	pub fn set_steering_mode(&self, mode: QueueMode) {
		self.inner.steering_queue.borrow_mut().mode = mode;
	}

	/// How queued steering messages are drained.
	pub fn steering_mode(&self) -> QueueMode {
		self.inner.steering_queue.borrow().mode
	}

	/// Controls how queued follow-up messages are drained.
	pub fn set_follow_up_mode(&self, mode: QueueMode) {
		self.inner.follow_up_queue.borrow_mut().mode = mode;
	}

	/// How queued follow-up messages are drained.
	pub fn follow_up_mode(&self) -> QueueMode {
		self.inner.follow_up_queue.borrow().mode
	}

	/// Queue a message to be injected after the current assistant turn finishes.
	pub fn steer(&self, message: AgentMessage) {
		self.inner.steering_queue.borrow_mut().enqueue(message);
	}

	/// Queue a message to run only after the agent would otherwise stop.
	pub fn follow_up(&self, message: AgentMessage) {
		self.inner.follow_up_queue.borrow_mut().enqueue(message);
	}

	/// Remove all queued steering messages.
	pub fn clear_steering_queue(&self) {
		self.inner.steering_queue.borrow_mut().clear();
	}

	/// Remove all queued follow-up messages.
	pub fn clear_follow_up_queue(&self) {
		self.inner.follow_up_queue.borrow_mut().clear();
	}

	/// Remove all queued steering and follow-up messages.
	pub fn clear_all_queues(&self) {
		self.clear_steering_queue();
		self.clear_follow_up_queue();
	}

	/// Returns true when either queue still contains pending messages.
	pub fn has_queued_messages(&self) -> bool {
		self.inner.steering_queue.borrow().has_items() || self.inner.follow_up_queue.borrow().has_items()
	}

	/// Preview the messages selected for the next turn without consuming them.
	pub fn peek_queued_messages(&self) -> Vec<AgentMessage> {
		let steering = self.inner.steering_queue.borrow().peek();
		if !steering.is_empty() {
			return steering;
		}
		self.inner.follow_up_queue.borrow().peek()
	}

	/// Active abort signal for the current run, if any.
	pub fn signal(&self) -> Option<AbortSignal> {
		self.inner
			.active_run
			.borrow()
			.as_ref()
			.map(|run| run.abort_controller.signal())
	}

	/// Abort the current run, if one is active.
	pub fn abort(&self) {
		let controller = self
			.inner
			.active_run
			.borrow()
			.as_ref()
			.map(|run| run.abort_controller.clone());
		if let Some(controller) = controller {
			controller.abort();
		}
	}

	/// Resolve when the current run and all awaited event listeners have finished.
	///
	/// This resolves after `agent_end` listeners settle.
	pub fn wait_for_idle(&self) -> LocalBoxFuture<'static, ()> {
		match self.inner.active_run.borrow().as_ref() {
			Some(run) => run.promise.clone().boxed_local(),
			None => Box::pin(async {}),
		}
	}

	/// Clear conversation state and queues while retaining the replayed prompt/tool baseline.
	pub fn reset(&self) -> PiResult<()> {
		if self.inner.active_run.borrow().is_some() {
			return Err(PiError::new(
				"Agent is already processing. Wait for completion before resetting.",
			));
		}

		{
			let mut state = self.inner.state.borrow_mut();
			let baseline = get_current_system_message(&state.messages);
			state.messages = baseline.map(|baseline| vec![baseline.into()]).unwrap_or_default();
			state.is_streaming = false;
			state.streaming_message = None;
			state.pending_tool_calls = IndexSet::new();
			state.error_message = None;
		}
		self.clear_follow_up_queue();
		self.clear_steering_queue();
		Ok(())
	}

	/// Start a new prompt from text, a single message, or a batch of messages.
	///
	/// The run starts during this call, like the synchronous part of the TS async method: the agent is streaming
	/// and `abort()` / `wait_for_idle()` see the run before the returned future is first polled.
	pub fn prompt(&self, input: impl Into<PromptInput>) -> LocalBoxFuture<'static, PiResult<()>> {
		if self.inner.active_run.borrow().is_some() {
			return rejected(
				"Agent is already processing a prompt. Use steer() or followUp() to queue messages, or wait for completion.",
			);
		}
		let messages = normalize_prompt_input(input.into());
		self.run_prompt_messages(messages, false)
	}

	/// Continue from the current transcript. The last message must be a user or tool-result message.
	///
	/// The checks, the queue drain and the start of the run happen during this call, as in `prompt`.
	pub fn r#continue(&self) -> LocalBoxFuture<'static, PiResult<()>> {
		if self.inner.active_run.borrow().is_some() {
			return rejected("Agent is already processing. Wait for completion before continuing.");
		}

		let last_role = {
			let state = self.inner.state.borrow();
			let last_role = state.messages.last().map(|message| message.role().to_string());
			if last_role.is_none() || state.messages.iter().all(|message| message.role() == "system") {
				return rejected("No messages to continue from");
			}
			last_role
		};

		if last_role.as_deref() == Some("assistant") {
			let queued_steering = self.inner.steering_queue.borrow_mut().drain();
			if !queued_steering.is_empty() {
				return self.run_prompt_messages(queued_steering, true);
			}

			let queued_follow_ups = self.inner.follow_up_queue.borrow_mut().drain();
			if !queued_follow_ups.is_empty() {
				return self.run_prompt_messages(queued_follow_ups, false);
			}

			return rejected("Cannot continue from message role: assistant");
		}

		self.run_continuation()
	}

	fn run_prompt_messages(
		&self,
		messages: Vec<AgentMessage>,
		skip_initial_steering_poll: bool,
	) -> LocalBoxFuture<'static, PiResult<()>> {
		self.run_with_lifecycle(|signal| {
			let context = self.create_context_snapshot();
			let config = self.create_loop_config(skip_initial_steering_poll);
			let stream_function = self.inner.properties.borrow().stream_function.clone();
			run_agent_loop(
				messages,
				context,
				config,
				self.event_sink(),
				Some(signal),
				Some(stream_function),
			)
		})
	}

	fn run_continuation(&self) -> LocalBoxFuture<'static, PiResult<()>> {
		self.run_with_lifecycle(|signal| {
			let context = self.create_context_snapshot();
			let config = self.create_loop_config(false);
			let stream_function = self.inner.properties.borrow().stream_function.clone();
			run_agent_loop_continue(context, config, self.event_sink(), Some(signal), Some(stream_function))
		})
	}

	fn event_sink(&self) -> AgentEventSink {
		let agent = self.clone();
		Rc::new(move |event| {
			let agent = agent.clone();
			Box::pin(async move { agent.process_events(event).await })
		})
	}

	fn create_context_snapshot(&self) -> AgentContext {
		let state = self.inner.state.borrow();
		AgentContext {
			messages: state.messages.clone(),
			tools: Some(state.tools.clone()),
		}
	}

	fn create_loop_config(&self, skip_initial_steering_poll: bool) -> AgentLoopConfig {
		let skip_initial_steering_poll = Rc::new(Cell::new(skip_initial_steering_poll));
		let properties = self.inner.properties.borrow().clone();
		let state = self.inner.state.borrow();
		let mut config = AgentLoopConfig::new(state.model.clone(), properties.convert_to_llm);
		config.reasoning = reasoning_for(state.thinking_level);
		config.session_id = properties.session_id;
		config.on_payload = properties.on_payload;
		config.on_response = properties.on_response;
		config.on_provider_stream_event = properties.on_provider_stream_event;
		config.transport = Some(properties.transport);
		config.thinking_budgets = properties.thinking_budgets;
		config.max_retry_delay_ms = properties.max_retry_delay_ms;
		config.identity = properties.identity;
		config.tool_execution = Some(properties.tool_execution);
		config.before_tool_call = properties.before_tool_call;
		config.after_tool_call = properties.after_tool_call;
		config.finish_turn = properties.finish_turn;
		config.prepare_request = properties.prepare_request;
		if properties.prepare_next_turn_with_context.is_some() || properties.prepare_next_turn.is_some() {
			let agent = self.clone();
			config.prepare_next_turn = Some(Rc::new(move |context| {
				let agent = agent.clone();
				Box::pin(async move {
					let (with_context, without_context) = {
						let properties = agent.inner.properties.borrow();
						(
							properties.prepare_next_turn_with_context.clone(),
							properties.prepare_next_turn.clone(),
						)
					};
					if let Some(prepare_next_turn_with_context) = with_context {
						return prepare_next_turn_with_context(context, agent.signal()).await;
					}
					match without_context {
						Some(prepare_next_turn) => prepare_next_turn(agent.signal()).await,
						None => Ok(None),
					}
				})
			}));
		}
		config.transform_context = properties.transform_context;
		config.get_api_key = properties.get_api_key;
		let agent = self.clone();
		config.get_steering_messages = Some(Rc::new(move || {
			let messages = if skip_initial_steering_poll.get() {
				skip_initial_steering_poll.set(false);
				Vec::new()
			} else {
				agent.inner.steering_queue.borrow_mut().drain()
			};
			Box::pin(async move { Ok(messages) })
		}));
		let agent = self.clone();
		config.get_follow_up_messages = Some(Rc::new(move || {
			let messages = agent.inner.follow_up_queue.borrow_mut().drain();
			Box::pin(async move { Ok(messages) })
		}));
		config
	}

	fn run_with_lifecycle<F, Fut>(&self, executor: F) -> LocalBoxFuture<'static, PiResult<()>>
	where
		F: FnOnce(AbortSignal) -> Fut,
		Fut: Future<Output = PiResult<Vec<AgentMessage>>> + 'static,
	{
		if self.inner.active_run.borrow().is_some() {
			return rejected("Agent is already processing.");
		}

		let abort_controller = AbortController::new();
		let (resolve, promise) = oneshot::channel::<()>();
		let promise: LocalBoxFuture<'static, ()> = Box::pin(promise.map(|_| ()));
		*self.inner.active_run.borrow_mut() = Some(ActiveRun {
			promise: promise.shared(),
			resolve,
			abort_controller: abort_controller.clone(),
		});
		let finish_run = FinishRunOnDrop(self.clone());

		{
			let mut state = self.inner.state.borrow_mut();
			state.is_streaming = true;
			state.streaming_message = None;
			state.error_message = None;
		}

		let signal = abort_controller.signal();
		let run = executor(signal.clone());
		let agent = self.clone();
		Box::pin(async move {
			let _finish_run = finish_run;
			match run.await {
				Ok(_) => Ok(()),
				Err(error) => agent.handle_run_failure(error, signal.aborted()).await,
			}
		})
	}

	async fn handle_run_failure(&self, error: PiError, aborted: bool) -> PiResult<()> {
		let failure_message: AgentMessage = {
			let state = self.inner.state.borrow();
			AssistantMessage {
				content: vec![AssistantContent::Text(TextContent::default())],
				api: state.model.api.clone(),
				provider: state.model.provider.clone(),
				model: state.model.id.clone(),
				usage: Usage::default(),
				stop_reason: if aborted {
					StopReason::Aborted
				} else {
					StopReason::Error
				},
				error_message: Some(error.message),
				timestamp: pi_host::now_ms(),
				..AssistantMessage::default()
			}
			.into()
		};
		self.process_events(AgentEvent::MessageStart {
			message: failure_message.clone(),
		})
		.await?;
		self.process_events(AgentEvent::MessageEnd {
			message: failure_message.clone(),
		})
		.await?;
		self.process_events(AgentEvent::TurnEnd {
			message: failure_message.clone(),
			tool_results: Vec::new(),
		})
		.await?;
		self.process_events(AgentEvent::AgentEnd {
			messages: vec![failure_message],
		})
		.await
	}

	fn finish_run(&self) {
		{
			let mut state = self.inner.state.borrow_mut();
			state.is_streaming = false;
			state.streaming_message = None;
			state.pending_tool_calls = IndexSet::new();
		}
		if let Some(run) = self.inner.active_run.borrow_mut().take() {
			let _ = run.resolve.send(());
		}
	}

	/// Reduce internal state for a loop event, then await listeners.
	///
	/// `agent_end` only means no further loop events will be emitted. The run is
	/// considered idle later, after all awaited listeners for `agent_end` finish
	/// and `finishRun()` clears runtime-owned state.
	async fn process_events(&self, event: AgentEvent) -> PiResult<()> {
		{
			let mut state = self.inner.state.borrow_mut();
			match &event {
				AgentEvent::MessageStart { message } => {
					state.streaming_message = Some(StreamingMessage::Message(message.clone()));
				}

				AgentEvent::MessageUpdate { message, .. } => {
					state.streaming_message = Some(StreamingMessage::Partial(message.clone()));
				}

				AgentEvent::MessageEnd { message } => {
					state.streaming_message = None;
					state.messages.push(message.clone());
				}

				AgentEvent::ToolExecutionStart { tool_call_id, .. } => {
					state.pending_tool_calls.insert(tool_call_id.clone());
				}

				AgentEvent::ToolExecutionEnd { tool_call_id, .. } => {
					state.pending_tool_calls.shift_remove(tool_call_id);
				}

				AgentEvent::TurnEnd {
					message: AgentMessage::Llm(Message::Assistant(message)),
					..
				} => {
					if let Some(error_message) = message.error_message.as_ref().filter(|message| !message.is_empty()) {
						state.error_message = Some(error_message.clone());
					}
				}

				AgentEvent::AgentEnd { .. } => {
					state.streaming_message = None;
				}

				_ => {}
			}
		}

		let Some(signal) = self.signal() else {
			return Err(PiError::new("Agent listener invoked outside active run"));
		};
		// A live iteration of the listener set: listeners added during the loop run, removed ones that were not
		// reached yet do not.
		let mut last_id: Option<u64> = None;
		loop {
			let next = self
				.inner
				.listeners
				.borrow()
				.iter()
				.find(|(id, _)| last_id.is_none_or(|last_id| *id > last_id))
				.cloned();
			let Some((id, listener)) = next else {
				break;
			};
			last_id = Some(id);
			listener(event.clone(), signal.clone()).await?;
		}
		Ok(())
	}
}

fn same_listener(left: &AgentListener, right: &AgentListener) -> bool {
	std::ptr::addr_eq(Rc::as_ptr(left), Rc::as_ptr(right))
}

struct FinishRunOnDrop(Agent);

impl Drop for FinishRunOnDrop {
	fn drop(&mut self) {
		self.0.finish_run();
	}
}

/// A rejected promise: the error a TS async method throws before its first `await`.
fn rejected(message: &str) -> LocalBoxFuture<'static, PiResult<()>> {
	Box::pin(futures::future::ready(Err(PiError::new(message))))
}

fn normalize_prompt_input(input: PromptInput) -> Vec<AgentMessage> {
	match input {
		PromptInput::Messages(messages) => messages,
		PromptInput::Message(message) => vec![message],
		PromptInput::Text { text, images } => {
			let mut content = vec![UserContent::Text(TextContent {
				text,
				..TextContent::default()
			})];
			if let Some(images) = images
				&& !images.is_empty()
			{
				content.extend(images.into_iter().map(UserContent::Image));
			}
			vec![
				UserMessage {
					content: UserMessageContent::Blocks(content),
					timestamp: pi_host::now_ms(),
					..UserMessage::default()
				}
				.into(),
			]
		}
	}
}
