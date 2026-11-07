//! Exports of `agent`: `AgentHandle`, the Rust `Agent` behind the facade's `Agent` class.
//!
//! The facade keeps the JS `state` with the caller's objects (model, tools, messages) and applies each event to it,
//! as the TS does; the handle keeps the Rust agent, which owns the queues and runs the loop. Before a run the facade
//! sends what changed in its state (`setModel`, `setThinkingLevel`, `spliceMessages`); the run (`prompt`,
//! `continue`) takes the tools, the callbacks and the options as they are at its start, and its event sink. The
//! handle holds those JS values only while the run is active, so an agent the app drops is collected with its
//! callbacks, which commonly reference the agent.

use std::rc::Rc;

use js_sys::{Array, Promise};
use pi_agent::{Agent, AgentInitialState, AgentMessage, AgentOptions, ConvertToLlmFn, PromptInput, StreamFn};
use pi_host::{LocalBoxFuture, PiError, PiResult};
use wasm_bindgen::prelude::*;

use super::bridge::{self, Bridge};
use super::events::{AgentEventSink, emitter};
use super::{get, optional};
use crate::options::{JsOptions, arg, throw};

/// A Rust `Agent` and the JS values of its active run.
#[wasm_bindgen]
pub struct AgentHandle {
	agent: Agent,
	bridge: Rc<Bridge>,
	default_convert_to_llm: ConvertToLlmFn,
}

#[wasm_bindgen]
impl AgentHandle {
	/// `new Agent({ initialState, steeringMode, followUpMode })`; the stream function and the callbacks come with
	/// each run.
	#[wasm_bindgen(constructor)]
	pub fn new(
		initial_state: JsValue,
		steering_mode: JsValue,
		follow_up_mode: JsValue,
	) -> Result<AgentHandle, JsValue> {
		let bridge = Rc::new(Bridge::default());
		let tools = get(&initial_state, "tools");
		let initial_state = AgentInitialState {
			system_prompt: throw(optional(get(&initial_state, "systemPrompt")))?,
			model: throw(optional(get(&initial_state, "model")))?,
			thinking_level: throw(optional(get(&initial_state, "thinkingLevel")))?,
			tools: if tools.is_undefined() || tools.is_null() {
				None
			} else {
				Some(throw(bridge.tools(&tools))?)
			},
			messages: throw(optional(get(&initial_state, "messages")))?,
		};
		let agent = throw(Agent::new(AgentOptions {
			initial_state: Some(initial_state),
			stream_fn: Some(unset_stream_fn()),
			steering_mode: throw(optional(steering_mode))?,
			follow_up_mode: throw(optional(follow_up_mode))?,
			..AgentOptions::default()
		}))?;
		let default_convert_to_llm = agent.properties().convert_to_llm.clone();
		let handle = AgentHandle {
			agent,
			bridge,
			default_convert_to_llm,
		};
		handle.release();
		Ok(handle)
	}

	/// The system message the agent put in front of the `given` initial messages, `undefined` when it put none.
	#[wasm_bindgen(js_name = insertedMessage)]
	pub fn inserted_message(&self, given: usize) -> Result<JsValue, JsValue> {
		let state = self.agent.state();
		match state.messages() {
			[first, ..] if state.messages().len() > given => throw(crate::json::to_js(first)),
			_ => Ok(JsValue::UNDEFINED),
		}
	}

	/// `state.model = model`.
	#[wasm_bindgen(js_name = setModel)]
	pub fn set_model(&self, model: JsValue) -> Result<(), JsValue> {
		self.agent.state_mut().model = arg(model)?;
		Ok(())
	}

	/// `state.thinkingLevel = level`.
	#[wasm_bindgen(js_name = setThinkingLevel)]
	pub fn set_thinking_level(&self, level: JsValue) -> Result<(), JsValue> {
		self.agent.state_mut().thinking_level = arg(level)?;
		Ok(())
	}

	/// `state.messages.splice(start, deleteCount, ...messages)`: the facade's changes to its messages since the last
	/// run.
	#[wasm_bindgen(js_name = spliceMessages)]
	pub fn splice_messages(&self, start: usize, delete_count: usize, messages: JsValue) -> Result<(), JsValue> {
		let messages: Vec<AgentMessage> = arg(messages)?;
		let mut state = self.agent.state_mut();
		let transcript = state.messages_mut();
		let start = start.min(transcript.len());
		let end = start.saturating_add(delete_count).min(transcript.len());
		transcript.splice(start..end, messages);
		Ok(())
	}

	/// `prompt(input, images)` with the run's options (`tools`, `streamFn` and the agent's public properties) and
	/// event sink.
	pub fn prompt(&self, input: JsValue, images: JsValue, options: JsValue, sink: AgentEventSink) -> Promise {
		let input = match input.as_string() {
			Some(text) => optional(images).map(|images| PromptInput::Text { text, images }),
			None if Array::is_array(&input) => self.bridge.messages_from_js(&input).map(PromptInput::Messages),
			None => self.message_from_js(input).map(PromptInput::Message),
		};
		match input {
			Ok(input) => self.run(options, sink, move |agent| agent.prompt(input)),
			Err(error) => rejected(error),
		}
	}

	/// `continue()` with the run's options and event sink.
	#[wasm_bindgen(js_name = continue)]
	pub fn continue_run(&self, options: JsValue, sink: AgentEventSink) -> Promise {
		self.run(options, sink, |agent| agent.r#continue())
	}

	/// `state.isStreaming` of the Rust agent: whether a run is active.
	#[wasm_bindgen(js_name = isStreaming)]
	pub fn is_streaming(&self) -> bool {
		self.agent.state().is_streaming()
	}

	/// `agent.signal`: the JS signal of the active run, the same object for the whole run.
	pub fn signal(&self) -> JsValue {
		self.bridge.signal(self.agent.signal().as_ref())
	}

	/// `abort()`.
	pub fn abort(&self) {
		self.agent.abort();
	}

	/// `reset()`; returns the baseline system message the transcript keeps, `undefined` when there is none.
	pub fn reset(&self) -> Result<JsValue, JsValue> {
		throw(self.agent.reset())?;
		match self.agent.state().messages().first() {
			Some(baseline) => throw(crate::json::to_js(baseline)),
			None => Ok(JsValue::UNDEFINED),
		}
	}

	/// `steer(message)`.
	pub fn steer(&self, message: JsValue) -> Result<(), JsValue> {
		self.agent.steer(throw(self.message_from_js(message))?);
		Ok(())
	}

	/// `followUp(message)`.
	#[wasm_bindgen(js_name = followUp)]
	pub fn follow_up(&self, message: JsValue) -> Result<(), JsValue> {
		self.agent.follow_up(throw(self.message_from_js(message))?);
		Ok(())
	}

	/// `clearSteeringQueue()`.
	#[wasm_bindgen(js_name = clearSteeringQueue)]
	pub fn clear_steering_queue(&self) {
		self.agent.clear_steering_queue();
	}

	/// `clearFollowUpQueue()`.
	#[wasm_bindgen(js_name = clearFollowUpQueue)]
	pub fn clear_follow_up_queue(&self) {
		self.agent.clear_follow_up_queue();
	}

	/// `hasQueuedMessages()`.
	#[wasm_bindgen(js_name = hasQueuedMessages)]
	pub fn has_queued_messages(&self) -> bool {
		self.agent.has_queued_messages()
	}

	/// `peekQueuedMessages()`.
	#[wasm_bindgen(js_name = peekQueuedMessages)]
	pub fn peek_queued_messages(&self) -> Result<JsValue, JsValue> {
		throw(crate::json::to_js(&self.agent.peek_queued_messages()))
	}

	/// `steeringMode`.
	#[wasm_bindgen(getter, js_name = steeringMode)]
	pub fn steering_mode(&self) -> Result<JsValue, JsValue> {
		throw(crate::json::to_js(&self.agent.steering_mode()))
	}

	/// `steeringMode = mode`.
	#[wasm_bindgen(setter, js_name = steeringMode)]
	pub fn set_steering_mode(&self, mode: JsValue) -> Result<(), JsValue> {
		self.agent.set_steering_mode(arg(mode)?);
		Ok(())
	}

	/// `followUpMode`.
	#[wasm_bindgen(getter, js_name = followUpMode)]
	pub fn follow_up_mode(&self) -> Result<JsValue, JsValue> {
		throw(crate::json::to_js(&self.agent.follow_up_mode()))
	}

	/// `followUpMode = mode`.
	#[wasm_bindgen(setter, js_name = followUpMode)]
	pub fn set_follow_up_mode(&self, mode: JsValue) -> Result<(), JsValue> {
		self.agent.set_follow_up_mode(arg(mode)?);
		Ok(())
	}
}

impl AgentHandle {
	/// The Rust message of a message the caller passed; its `message_start`/`message_end` carry the caller's object, as
	/// in the TS. The bridge keeps the object until the end of the next run, for a message queued between runs.
	fn message_from_js(&self, message: JsValue) -> PiResult<AgentMessage> {
		let value: AgentMessage = crate::json::from_js(message.clone())?;
		self.bridge.record_message(value.clone(), message);
		Ok(value)
	}

	/// Starts a run with `options` and `sink`. A call the agent rejects (a run is active, nothing to continue) leaves
	/// the active run's values in place and returns the rejection.
	fn run(
		&self,
		options: JsValue,
		sink: AgentEventSink,
		start: impl FnOnce(&Agent) -> LocalBoxFuture<'static, PiResult<()>>,
	) -> Promise {
		if self.agent.signal().is_some() {
			return promise(start(&self.agent));
		}
		if let Err(error) = self.configure(&options) {
			self.release();
			return rejected(error);
		}
		let emit = emitter(sink, self.bridge.clone());
		let unsubscribe = self.agent.subscribe(Rc::new(move |event, _signal| emit(event)));
		let run = start(&self.agent);
		if self.agent.signal().is_none() {
			unsubscribe();
			self.release();
			return promise(run);
		}
		let handle = self.detached();
		promise(Box::pin(async move {
			let result = run.await;
			unsubscribe();
			handle.release();
			result
		}))
	}

	/// The agent's tools and properties for a run, from the facade's `options` object.
	fn configure(&self, options: &JsValue) -> PiResult<()> {
		let tools = self.bridge.tools(&get(options, "tools"))?;
		let mut state = self.agent.state_mut();
		state.set_tools(tools);
		self.bridge.seed(
			state.messages(),
			&get(options, "messages"),
			&state.model,
			get(options, "model"),
		);
		drop(state);
		let mut options = JsOptions::new(options.clone());
		let bridge = &self.bridge;
		let mut properties = self.agent.properties_mut();
		properties.stream_function = match options.function("streamFn") {
			Some(stream_fn) => bridge::stream_fn(stream_fn, Some(bridge.clone())),
			None => return Err(PiError::type_error("streamFn is not a function")),
		};
		properties.convert_to_llm = match options.function("convertToLlm") {
			Some(convert_to_llm) => bridge::convert_to_llm(bridge.clone(), convert_to_llm),
			None => self.default_convert_to_llm.clone(),
		};
		properties.transform_context = options
			.function("transformContext")
			.map(|f| bridge::transform_context(bridge.clone(), f));
		properties.get_api_key = options.function("getApiKey").map(bridge::get_api_key);
		properties.on_payload = options.function("onPayload").map(bridge::on_payload);
		properties.on_response = options.function("onResponse").map(bridge::on_response);
		properties.on_provider_stream_event = options
			.function("onProviderStreamEvent")
			.map(bridge::on_provider_stream_event);
		properties.before_tool_call = options
			.function("beforeToolCall")
			.map(|f| bridge::before_tool_call(bridge.clone(), f));
		properties.after_tool_call = options
			.function("afterToolCall")
			.map(|f| bridge::after_tool_call(bridge.clone(), f));
		properties.finish_turn = options
			.function("finishTurn")
			.map(|f| bridge::finish_turn(bridge.clone(), f));
		properties.prepare_request = options
			.function("prepareRequest")
			.map(|f| bridge::prepare_request(bridge.clone(), f));
		properties.prepare_next_turn = options
			.function("prepareNextTurn")
			.map(|f| bridge::agent_prepare_next_turn(bridge.clone(), f));
		properties.prepare_next_turn_with_context = options
			.function("prepareNextTurnWithContext")
			.map(|f| bridge::agent_prepare_next_turn_with_context(bridge.clone(), f));
		properties.session_id = options.take("sessionId");
		properties.thinking_budgets = options.take("thinkingBudgets");
		properties.transport = options.take("transport").unwrap_or(pi_ai::Transport::Auto);
		properties.max_retry_delay_ms = options.take("maxRetryDelayMs");
		properties.tool_execution = options
			.take("toolExecution")
			.unwrap_or(pi_agent::ToolExecutionMode::Parallel);
		properties.identity = options.take("identity");
		Ok(())
	}

	/// Drops the JS values of the last run: tools, callbacks and the run's JS signal.
	fn release(&self) {
		self.agent.state_mut().set_tools(Vec::new());
		let mut properties = self.agent.properties_mut();
		properties.stream_function = unset_stream_fn();
		properties.convert_to_llm = self.default_convert_to_llm.clone();
		properties.transform_context = None;
		properties.get_api_key = None;
		properties.on_payload = None;
		properties.on_response = None;
		properties.on_provider_stream_event = None;
		properties.before_tool_call = None;
		properties.after_tool_call = None;
		properties.finish_turn = None;
		properties.prepare_request = None;
		properties.prepare_next_turn = None;
		properties.prepare_next_turn_with_context = None;
		drop(properties);
		self.bridge.release();
	}

	fn detached(&self) -> AgentHandle {
		AgentHandle {
			agent: self.agent.clone(),
			bridge: self.bridge.clone(),
			default_convert_to_llm: self.default_convert_to_llm.clone(),
		}
	}
}

/// `getCurrentSystemPrompt(messages)` for the facade's `state.systemPrompt`.
#[wasm_bindgen(js_name = agentCurrentSystemPrompt)]
pub fn current_system_prompt(messages: JsValue) -> Result<String, JsValue> {
	let messages: Vec<AgentMessage> = arg(messages)?;
	Ok(pi_ai::utils::transcript::get_current_system_prompt(&messages))
}

/// The stream function of an agent between runs; each run sets the facade's.
fn unset_stream_fn() -> StreamFn {
	Rc::new(|_, _, _| Box::pin(async { Err(PiError::new("Agent stream function is set per run")) }))
}

fn promise(run: LocalBoxFuture<'static, PiResult<()>>) -> Promise {
	wasm_bindgen_futures::future_to_promise(async move {
		run.await
			.map(|()| JsValue::UNDEFINED)
			.map_err(|error| crate::error::to_js(&error))
	})
}

fn rejected(error: PiError) -> Promise {
	Promise::reject(&crate::error::to_js(&error))
}
