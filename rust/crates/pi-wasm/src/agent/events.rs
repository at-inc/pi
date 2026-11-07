//! Agent events across the boundary: every `AgentEvent` of a run goes to a JS `AgentEventSink` as a change, not as a
//! snapshot.
//!
//! - `message_update` goes through the sink's `updates`, an assistant message sink (`crate::events`): a text delta is
//!   an append, and JS keeps one live partial per response. `update()` then emits the event over that partial.
//! - `message_start`, `message_end` and the tool events carry their own JSON. A message the run read from JS (a
//!   prompt, a steering message) goes as that object instead, so the listeners see the caller's object, as in the TS;
//!   the object JS makes of any other ended message is kept in the run's `Bridge`, so later callbacks receive it.
//!   `tool_execution_update` leaves out `args` when they are the ones of its `tool_execution_start`.
//! - `turn_end` and `agent_end` refer to the messages of the run's `message_end` events by index, so JS passes the
//!   objects it already holds (the ones in `state.messages`) and no message crosses twice.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use js_sys::Promise;
use pi_agent::{AgentEvent, AgentMessage};
use pi_ai::{Message, ToolResultMessage};
use pi_host::{LocalBoxFuture, PiError, PiResult};
use serde::Serialize;
use serde_json::{Value, json};
use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::JsFuture;

use super::bridge::Bridge;
use crate::events::{EventSink, Mirror};

/// Event kinds of `emit`, in `AgentEvent` order without `message_update`.
const AGENT_START: u32 = 0;
const AGENT_END: u32 = 1;
const TURN_START: u32 = 2;
const TURN_END: u32 = 3;
const MESSAGE_START: u32 = 4;
const MESSAGE_END: u32 = 5;
const TOOL_EXECUTION_START: u32 = 6;
const TOOL_EXECUTION_UPDATE: u32 = 7;
const TOOL_EXECUTION_END: u32 = 8;

#[wasm_bindgen(typescript_custom_section)]
const AGENT_EVENT_SINK: &str = r#"
/** Receives one agent run's events from the core; see `agent/events.rs`. */
export interface AgentEventSink {
	/** The streamed response's changes; `update()` emits the event just sent here as `message_update`. */
	readonly updates: AssistantMessageEventSink;
	/**
	 * One event other than `message_update`, with its JSON: the message of `message_start`/`message_end`, the
	 * event of the tool events, `[message, toolResults]` of `turn_end` and the messages of `agent_end`, where a
	 * number is the index of a message among the run's `message_end` messages. A `message_start`/`message_end`
	 * whose message JS already holds has no JSON and passes that `message`. Returns the listeners' completion.
	 */
	emit(kind: number, data: string | undefined, message: unknown): Promise<void> | void;
	/** The message object of the last `message_end`. */
	readonly ended: unknown;
	/** `message_update` for the event just sent to `updates`. */
	update(): Promise<void> | void;
}
"#;

#[wasm_bindgen]
extern "C" {
	/// The JS receiver of a run's events (`AgentEvents` in the facade).
	#[wasm_bindgen(typescript_type = "AgentEventSink")]
	pub type AgentEventSink;
	#[wasm_bindgen(method, getter)]
	fn updates(this: &AgentEventSink) -> EventSink;
	#[wasm_bindgen(method, catch)]
	fn emit(this: &AgentEventSink, kind: u32, data: Option<String>, message: &JsValue) -> Result<JsValue, JsValue>;
	#[wasm_bindgen(method, getter)]
	fn ended(this: &AgentEventSink) -> JsValue;
	#[wasm_bindgen(method, catch)]
	fn update(this: &AgentEventSink) -> Result<JsValue, JsValue>;
}

/// The emit function of a run (`pi_agent::AgentEventSink`): sends each event to `sink` and awaits the JS listeners.
pub fn emitter(sink: AgentEventSink, bridge: Rc<Bridge>) -> pi_agent::AgentEventSink {
	let encoder = Rc::new(RefCell::new(Encoder::new(sink, bridge)));
	Rc::new(move |event| {
		let pending = encoder.borrow_mut().send(&event);
		Box::pin(settle(pending))
	})
}

/// Awaits what a JS callback returned when it is a promise.
pub fn settle(pending: PiResult<JsValue>) -> LocalBoxFuture<'static, PiResult<()>> {
	Box::pin(async move {
		let pending = pending?;
		if pending.is_undefined() {
			return Ok(());
		}
		JsFuture::from(Promise::resolve(&pending))
			.await
			.map(|_| ())
			.map_err(crate::error::from_js)
	})
}

/// What the JS side holds of the run, so each event sends only what it does not have.
pub struct Encoder {
	sink: AgentEventSink,
	bridge: Rc<Bridge>,
	updates: EventSink,
	mirror: Mirror,
	/// The messages of the run's `message_end` events, in order.
	ended: Vec<AgentMessage>,
	/// The `args` of each running tool call's `tool_execution_start`.
	tool_args: HashMap<String, Value>,
}

/// A message of `turn_end`/`agent_end`: the index of an ended message, or the message.
#[derive(Serialize)]
#[serde(untagged)]
enum Ref<'a, M> {
	Ended(usize),
	Message(&'a M),
}

impl Encoder {
	pub fn new(sink: AgentEventSink, bridge: Rc<Bridge>) -> Self {
		Self {
			updates: sink.updates(),
			sink,
			bridge,
			mirror: Mirror::default(),
			ended: Vec::new(),
			tool_args: HashMap::new(),
		}
	}

	/// Sends `event` and returns what the sink returned (the listeners' promise, if any).
	pub fn send(&mut self, event: &AgentEvent) -> PiResult<JsValue> {
		use AgentEvent as E;
		let (kind, data) = match event {
			E::AgentStart => {
				self.ended.clear();
				(AGENT_START, None)
			}
			E::AgentEnd { messages } => {
				let refs: Vec<_> = messages.iter().map(|message| self.reference(message)).collect();
				(AGENT_END, Some(to_json(&refs)?))
			}
			E::TurnStart => (TURN_START, None),
			E::TurnEnd { message, tool_results } => {
				let results: Vec<_> = tool_results.iter().map(|result| self.tool_result(result)).collect();
				(TURN_END, Some(to_json(&(self.reference(message), results))?))
			}
			E::MessageStart { message } => return self.send_message(MESSAGE_START, message),
			E::MessageUpdate {
				assistant_message_event,
				..
			} => {
				self.mirror.send(&self.updates, assistant_message_event);
				return self.sink.update().map_err(crate::error::from_js);
			}
			E::MessageEnd { message } => {
				self.mirror.end(&self.updates);
				self.ended.push(message.clone());
				return self.send_message(MESSAGE_END, message);
			}
			E::ToolExecutionStart { tool_call_id, args, .. } => {
				self.tool_args.insert(tool_call_id.clone(), args.clone());
				(TOOL_EXECUTION_START, Some(to_json(event)?))
			}
			E::ToolExecutionUpdate {
				tool_call_id,
				tool_name,
				args,
				partial_result,
			} => {
				let data = if self.tool_args.get(tool_call_id) == Some(args) {
					to_json(&json!({
						"type": "tool_execution_update",
						"toolCallId": tool_call_id,
						"toolName": tool_name,
						"partialResult": partial_result,
					}))?
				} else {
					to_json(event)?
				};
				(TOOL_EXECUTION_UPDATE, Some(data))
			}
			E::ToolExecutionEnd { tool_call_id, .. } => {
				self.tool_args.remove(tool_call_id);
				(TOOL_EXECUTION_END, Some(to_json(event)?))
			}
		};
		self.sink
			.emit(kind, data, &JsValue::UNDEFINED)
			.map_err(crate::error::from_js)
	}

	/// `message_start` or `message_end`: the JS object of `message` when the run has one, else its JSON, and the
	/// object JS made of an ended message is kept for the run's callbacks.
	fn send_message(&self, kind: u32, message: &AgentMessage) -> PiResult<JsValue> {
		if let Some(object) = self.bridge.known_message(message) {
			return self.sink.emit(kind, None, &object).map_err(crate::error::from_js);
		}
		let pending = self
			.sink
			.emit(kind, Some(to_json(message)?), &JsValue::UNDEFINED)
			.map_err(crate::error::from_js)?;
		if kind == MESSAGE_END {
			self.bridge.record_message(message.clone(), self.sink.ended());
		}
		Ok(pending)
	}

	fn reference<'a>(&self, message: &'a AgentMessage) -> Ref<'a, AgentMessage> {
		match self.ended.iter().rposition(|ended| ended == message) {
			Some(index) => Ref::Ended(index),
			None => Ref::Message(message),
		}
	}

	fn tool_result<'a>(&self, message: &'a ToolResultMessage) -> Ref<'a, ToolResultMessage> {
		let ended = self
			.ended
			.iter()
			.rposition(|ended| matches!(ended, AgentMessage::Llm(Message::ToolResult(ended)) if ended == message));
		match ended {
			Some(index) => Ref::Ended(index),
			None => Ref::Message(message),
		}
	}
}

fn to_json<T: Serialize + ?Sized>(value: &T) -> PiResult<String> {
	serde_json::to_string(value).map_err(|error| PiError::type_error(error.to_string()))
}
