//! Assistant message event streams across the boundary.
//!
//! Rust to JS (`send`): every event of a Rust `AssistantMessageEventStream` is passed to a JS `EventSink` as its own
//! fields plus the change to the live `partial`, never the whole partial: a text or thinking delta is an append, a
//! tool call delta marks the arguments stale (the sink reads them from the `AssistantMessageHandle` when JS reads
//! them), a block start carries that block with its text or thinking empty (the deltas after it append), a block end
//! carries the whole block, and the message fields other than `content` are sent when they changed. The sink keeps
//! one JS `AssistantMessage` per stream and applies each change, so every event's `partial` is the same live object,
//! as in the TS. Its text is the text up to that event, while the Rust partial may already hold the deltas of a
//! network chunk JS has not taken yet; `done` and `error` carry the whole final message once.
//!
//! JS to Rust (`from_js`): a JS `AssistantMessageEventStream` (any async iterable of events with `result()`) becomes
//! a Rust stream whose shared partial is kept in sync the same way: deltas are appended, blocks are read at their
//! start and end events, the message at `start`, `done` and `error`.

use std::cell::RefCell;
use std::rc::Rc;

use futures::FutureExt;
use js_sys::{Function, Object, Promise, Reflect};
use pi_ai::{
	AssistantContent, AssistantMessage, AssistantMessageEvent, AssistantMessageEventStream, SharedAssistantMessage,
	StopReason,
};
use serde_json::Value;
use wasm_bindgen::JsCast;
use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::JsFuture;

/// Event kinds, in `AssistantMessageEvent` order. `END` is the end of the stream (`stream.end(result)`).
pub const START: u32 = 0;
pub const TEXT_START: u32 = 1;
pub const TEXT_DELTA: u32 = 2;
pub const TEXT_END: u32 = 3;
pub const THINKING_START: u32 = 4;
pub const THINKING_DELTA: u32 = 5;
pub const THINKING_END: u32 = 6;
pub const TOOLCALL_START: u32 = 7;
pub const TOOLCALL_DELTA: u32 = 8;
pub const TOOLCALL_END: u32 = 9;
pub const DONE: u32 = 10;
pub const ERROR: u32 = 11;
pub const END: u32 = 12;

#[wasm_bindgen(typescript_custom_section)]
const EVENT_SINK: &str = r#"
/** Receives one stream's events from the core; see `events.rs`. */
export interface AssistantMessageEventSink {
	event(
		kind: number,
		index: number,
		text: string | undefined,
		head: string | undefined,
		block: string | undefined,
		message: string | undefined,
	): void;
	attach(partial: AssistantMessageHandle): void;
	sync(message: string): void;
}
"#;

#[wasm_bindgen]
extern "C" {
	/// The JS receiver of one stream's events (`AssistantMessageEventSink` in `@at-inc/pi-core`).
	#[wasm_bindgen(typescript_type = "AssistantMessageEventSink")]
	pub type EventSink;
	/// One event: `text` is the delta of a delta event and the reason of `done`/`error`; `head` is the JSON of the
	/// message without `content` when it changed; `block` is the JSON of block `index` when JS must replace it;
	/// `message` is the JSON of a whole message for `start`, `done`, `error` and `end`, or when the partial was
	/// replaced. For `done`/`error`, `index` is 1 when the final message is the live partial.
	#[wasm_bindgen(method, catch)]
	fn event(
		this: &EventSink,
		kind: u32,
		index: u32,
		text: Option<&str>,
		head: Option<String>,
		block: Option<String>,
		message: Option<String>,
	) -> Result<(), JsValue>;
	/// Gives the sink the handle of the live partial, before the first event.
	#[wasm_bindgen(method, catch)]
	fn attach(this: &EventSink, partial: AssistantMessageHandle) -> Result<(), JsValue>;
	/// Makes the sink's partial equal `message` (JSON), after the last event of a `Forwarder`.
	#[wasm_bindgen(method, catch)]
	fn sync(this: &EventSink, message: String) -> Result<(), JsValue>;

	#[wasm_bindgen(js_namespace = console, js_name = error)]
	fn console_error(value: &JsValue);
}

/// The live partial of a stream, for the sink's lazy reads.
#[wasm_bindgen]
pub struct AssistantMessageHandle {
	partial: SharedAssistantMessage,
}

#[wasm_bindgen]
impl AssistantMessageHandle {
	/// The JSON of the current `arguments` of tool call block `index`, `undefined` when that block is not a tool call.
	#[wasm_bindgen(js_name = toolCallArguments)]
	pub fn tool_call_arguments(&self, index: usize) -> Option<String> {
		match self.partial.borrow().content.get(index) {
			Some(AssistantContent::ToolCall(call)) => serde_json::to_string(&call.arguments).ok(),
			_ => None,
		}
	}
}

/// Sends every event of `stream` to `sink` as it arrives, then `END` with the stream's result when it ended without a
/// `done` or `error` event.
pub fn send(stream: AssistantMessageEventStream, sink: EventSink) {
	wasm_bindgen_futures::spawn_local(async move {
		let mut mirror = Mirror::default();
		while let Some(event) = stream.next().await {
			mirror.send(&sink, &event);
		}
		let result = if mirror.finished {
			None
		} else {
			stream
				.result()
				.now_or_never()
				.flatten()
				.and_then(|message| serde_json::to_string(&message).ok())
		};
		report(sink.event(END, 0, None, None, None, result));
	});
}

/// Sends the events a Rust function pushes into `stream` to `sink` when asked, for a function that fills a message and a
/// stream the caller owns (`processResponsesStream`): `flush` sends the queued events at once, so each JS event sees
/// the message as it was when the function took its next input; `finish` flushes and then syncs the whole message.
pub struct Forwarder {
	stream: AssistantMessageEventStream,
	sink: EventSink,
	mirror: Mirror,
}

impl Forwarder {
	/// Forwards the events of `stream` to `sink`.
	pub fn new(stream: AssistantMessageEventStream, sink: EventSink) -> Self {
		Self {
			stream,
			sink,
			mirror: Mirror::default(),
		}
	}

	/// Sends the queued events.
	pub fn flush(&mut self) {
		while let Some(Some(event)) = self.stream.next().now_or_never() {
			self.mirror.send(&self.sink, &event);
		}
	}

	/// Sends the queued events, then makes the sink's partial equal `message`.
	pub fn finish(&mut self, message: &SharedAssistantMessage) {
		self.flush();
		report(self.sink.sync(whole(message)));
	}
}

/// What the JS side holds of the partial, so each event sends only what changed.
#[derive(Default)]
struct Mirror {
	partial: Option<SharedAssistantMessage>,
	/// The message without `content`, as last sent.
	head: Option<AssistantMessage>,
	/// Per block: the byte length of its text or thinking as JS has it.
	lengths: Vec<usize>,
	/// Per block: the tool call `id` and `name` as JS has them.
	tool_heads: Vec<Option<(String, String)>>,
	finished: bool,
}

impl Mirror {
	fn send(&mut self, sink: &EventSink, event: &AssistantMessageEvent) {
		use AssistantMessageEvent as E;
		let (kind, index, partial, delta) = match event {
			E::Start { partial } => (START, 0, partial, None),
			E::TextStart { content_index, partial } => (TEXT_START, *content_index, partial, None),
			E::TextDelta {
				content_index,
				delta,
				partial,
			} => (TEXT_DELTA, *content_index, partial, Some(delta)),
			E::TextEnd {
				content_index, partial, ..
			} => (TEXT_END, *content_index, partial, None),
			E::ThinkingStart { content_index, partial } => (THINKING_START, *content_index, partial, None),
			E::ThinkingDelta {
				content_index,
				delta,
				partial,
			} => (THINKING_DELTA, *content_index, partial, Some(delta)),
			E::ThinkingEnd {
				content_index, partial, ..
			} => (THINKING_END, *content_index, partial, None),
			E::ToolcallStart { content_index, partial } => (TOOLCALL_START, *content_index, partial, None),
			E::ToolcallDelta {
				content_index,
				delta,
				partial,
			} => (TOOLCALL_DELTA, *content_index, partial, Some(delta)),
			E::ToolcallEnd {
				content_index, partial, ..
			} => (TOOLCALL_END, *content_index, partial, None),
			E::Done { reason, message } => return self.finish(sink, DONE, *reason, message),
			E::Error { reason, error } => return self.finish(sink, ERROR, *reason, error),
		};
		let replaced = !self.partial.as_ref().is_some_and(|live| Rc::ptr_eq(live, partial));
		if replaced {
			self.reset(sink, partial);
		}
		let message = replaced.then(|| whole(partial));
		let head = if replaced { None } else { self.head_change(partial) };
		let block = match kind {
			START => None,
			TEXT_START | THINKING_START => self.started_block(partial, index),
			TEXT_DELTA | THINKING_DELTA => (!self.appended(partial, index, delta.map_or("", String::as_str)))
				.then(|| self.block(partial, index))
				.flatten(),
			TOOLCALL_DELTA => (!self.same_tool_head(partial, index))
				.then(|| self.block(partial, index))
				.flatten(),
			_ => self.block(partial, index),
		};
		report(sink.event(kind, index as u32, delta.map(String::as_str), head, block, message));
	}

	fn finish(&mut self, sink: &EventSink, kind: u32, reason: StopReason, message: &SharedAssistantMessage) {
		self.finished = true;
		let live = self.partial.as_ref().is_some_and(|live| Rc::ptr_eq(live, message));
		let reason = serde_json::to_value(reason).ok();
		let reason = reason.as_ref().and_then(Value::as_str);
		report(sink.event(kind, u32::from(live), reason, None, None, Some(whole(message))));
	}

	fn reset(&mut self, sink: &EventSink, partial: &SharedAssistantMessage) {
		*self = Mirror::default();
		self.partial = Some(partial.clone());
		let mut head = partial.borrow().clone();
		head.content.clear();
		self.head = Some(head);
		let message = partial.borrow();
		for (index, block) in message.content.iter().enumerate() {
			self.track(index, block);
		}
		drop(message);
		report(sink.attach(AssistantMessageHandle {
			partial: partial.clone(),
		}));
	}

	/// The JSON of the message without `content` when it differs from what JS has.
	fn head_change(&mut self, partial: &SharedAssistantMessage) -> Option<String> {
		let mut message = partial.borrow_mut();
		let content = std::mem::take(&mut message.content);
		let change = (self.head.as_ref() != Some(&*message)).then(|| {
			self.head = Some(message.clone());
			serde_json::to_string(&*message).unwrap_or_default()
		});
		message.content = content;
		change
	}

	/// Whether block `index` continues with `delta` right after the part JS has; records the new length. The core
	/// parses a whole network chunk before JS takes the first of its events, so the live text may already hold the
	/// deltas of later events, which those events append in turn.
	fn appended(&mut self, partial: &SharedAssistantMessage, index: usize, delta: &str) -> bool {
		let message = partial.borrow();
		let text = match message.content.get(index) {
			Some(AssistantContent::Text(block)) => &block.text,
			Some(AssistantContent::Thinking(block)) => &block.thinking,
			_ => return false,
		};
		let Some(known) = self.lengths.get(index).copied() else {
			return false;
		};
		let end = known + delta.len();
		if text.get(known..end) != Some(delta) {
			return false;
		}
		self.lengths[index] = end;
		true
	}

	/// Whether tool call block `index` still has the `id` and `name` JS has.
	fn same_tool_head(&self, partial: &SharedAssistantMessage, index: usize) -> bool {
		match (partial.borrow().content.get(index), self.tool_heads.get(index)) {
			(Some(AssistantContent::ToolCall(call)), Some(Some((id, name)))) => call.id == *id && call.name == *name,
			_ => false,
		}
	}

	/// The JSON of a block at its start event, with its text or thinking left out: the live block may already hold the
	/// deltas of later events, and each of those events appends its own. A block that started with text gets resent
	/// whole by its first delta, as `appended` then fails.
	fn started_block(&mut self, partial: &SharedAssistantMessage, index: usize) -> Option<String> {
		let mut block = partial.borrow().content.get(index)?.clone();
		match &mut block {
			AssistantContent::Text(text) => text.text.clear(),
			AssistantContent::Thinking(thinking) => thinking.thinking.clear(),
			AssistantContent::ToolCall(_) => {}
		}
		self.track(index, &block);
		serde_json::to_string(&block).ok()
	}

	/// The JSON of block `index`, recorded as what JS now has.
	fn block(&mut self, partial: &SharedAssistantMessage, index: usize) -> Option<String> {
		let message = partial.borrow();
		let block = message.content.get(index)?;
		self.track(index, block);
		serde_json::to_string(block).ok()
	}

	/// Records `block` as what JS has at `index`.
	fn track(&mut self, index: usize, block: &AssistantContent) {
		if self.lengths.len() <= index {
			self.lengths.resize(index + 1, 0);
			self.tool_heads.resize(index + 1, None);
		}
		let (length, tool_head) = match block {
			AssistantContent::Text(block) => (block.text.len(), None),
			AssistantContent::Thinking(block) => (block.thinking.len(), None),
			AssistantContent::ToolCall(call) => (0, Some((call.id.clone(), call.name.clone()))),
		};
		self.lengths[index] = length;
		self.tool_heads[index] = tool_head;
	}
}

fn whole(message: &SharedAssistantMessage) -> String {
	serde_json::to_string(&*message.borrow()).unwrap_or_default()
}

/// A throwing sink is a facade bug; it must not unwind through the core's tasks.
fn report(result: Result<(), JsValue>) {
	if let Err(error) = result {
		console_error(&error);
	}
}

/// A Rust stream fed by a JS `AssistantMessageEventStream`: iterates it with `for await`, keeps a Rust partial in sync
/// (see the module docs), and ends with `await source.result()` when the source ended without `done`/`error`, like
/// `lazyStream`'s forwarding. `signal` is the JS signal passed to the JS producer, kept alive until its stream ends.
pub fn from_js(source: JsValue, signal: Option<crate::abort::JsSignal>) -> AssistantMessageEventStream {
	let stream = AssistantMessageEventStream::new();
	let target = stream.clone();
	wasm_bindgen_futures::spawn_local(async move {
		let _signal = signal;
		let mut reader = JsEvents::default();
		if let Err(error) = reader.forward(&source, &target).await {
			target.push(AssistantMessageEvent::Error {
				reason: StopReason::Error,
				error: Rc::new(RefCell::new(AssistantMessage {
					stop_reason: StopReason::Error,
					error_message: Some(error.message),
					..AssistantMessage::default()
				})),
			});
		}
	});
	stream
}

#[derive(Default)]
struct JsEvents {
	partial: Option<(JsValue, SharedAssistantMessage)>,
	finished: bool,
}

impl JsEvents {
	async fn forward(&mut self, source: &JsValue, target: &AssistantMessageEventStream) -> pi_host::PiResult<()> {
		let iterator_fn: Function = Reflect::get(source, &js_sys::Symbol::async_iterator())
			.map_err(crate::error::from_js)?
			.unchecked_into();
		let iterator = iterator_fn.call0(source).map_err(crate::error::from_js)?;
		let next: Function = get(&iterator, "next").unchecked_into();
		loop {
			let step = next.call0(&iterator).map_err(crate::error::from_js)?;
			let step = JsFuture::from(Promise::resolve(&step))
				.await
				.map_err(crate::error::from_js)?;
			if get(&step, "done").is_truthy() {
				break;
			}
			if let Some(event) = self.event(&get(&step, "value"))? {
				target.push(event);
			}
		}
		if !self.finished {
			let result = get(source, "result");
			if let Some(result) = result.dyn_ref::<Function>() {
				let message = result.call0(source).map_err(crate::error::from_js)?;
				let message = JsFuture::from(Promise::resolve(&message))
					.await
					.map_err(crate::error::from_js)?;
				target.end(Some(crate::json::from_js(message)?));
				return Ok(());
			}
			target.end(None);
		}
		Ok(())
	}

	fn event(&mut self, event: &JsValue) -> pi_host::PiResult<Option<AssistantMessageEvent>> {
		use AssistantMessageEvent as E;
		let kind = get(event, "type").as_string().unwrap_or_default();
		if kind == "done" || kind == "error" {
			self.finished = true;
			let field = if kind == "done" { "message" } else { "error" };
			let js_message = get(event, field);
			let message: AssistantMessage = crate::json::from_js(js_message.clone())?;
			let shared = match &self.partial {
				Some((js, shared)) if js == &js_message => {
					*shared.borrow_mut() = message;
					shared.clone()
				}
				_ => Rc::new(RefCell::new(message)),
			};
			let reason = crate::json::from_js(get(event, "reason")).unwrap_or(StopReason::Error);
			return Ok(Some(if kind == "done" {
				E::Done {
					reason,
					message: shared,
				}
			} else {
				E::Error { reason, error: shared }
			}));
		}
		let js_partial = get(event, "partial");
		let partial = match &self.partial {
			Some((js, shared)) if js == &js_partial => shared.clone(),
			_ => {
				let shared: SharedAssistantMessage = Rc::new(RefCell::new(crate::json::from_js(js_partial.clone())?));
				self.partial = Some((js_partial.clone(), shared.clone()));
				if kind != "start" {
					// A partial first seen mid-stream was read whole above.
					return Ok(self.block_event(&kind, event, &shared, false));
				}
				shared
			}
		};
		if kind == "start" {
			*partial.borrow_mut() = crate::json::from_js(js_partial)?;
			return Ok(Some(E::Start { partial }));
		}
		Ok(self.block_event(&kind, event, &partial, true))
	}

	/// A block event; with `sync`, applies its change to the Rust partial first.
	fn block_event(
		&mut self,
		kind: &str,
		event: &JsValue,
		partial: &SharedAssistantMessage,
		sync: bool,
	) -> Option<AssistantMessageEvent> {
		use AssistantMessageEvent as E;
		let content_index = get(event, "contentIndex").as_f64().unwrap_or(0.0) as usize;
		let delta = get(event, "delta").as_string().unwrap_or_default();
		if sync {
			match kind {
				"text_delta" | "thinking_delta" => {
					let mut message = partial.borrow_mut();
					match message.content.get_mut(content_index) {
						Some(AssistantContent::Text(block)) => block.text.push_str(&delta),
						Some(AssistantContent::Thinking(block)) => block.thinking.push_str(&delta),
						_ => {}
					}
				}
				"toolcall_delta" => {}
				_ => self.read_block(event, partial, content_index),
			}
		}
		let partial = partial.clone();
		Some(match kind {
			"text_start" => E::TextStart { content_index, partial },
			"text_delta" => E::TextDelta {
				content_index,
				delta,
				partial,
			},
			"text_end" => E::TextEnd {
				content_index,
				content: get(event, "content").as_string().unwrap_or_default(),
				partial,
			},
			"thinking_start" => E::ThinkingStart { content_index, partial },
			"thinking_delta" => E::ThinkingDelta {
				content_index,
				delta,
				partial,
			},
			"thinking_end" => E::ThinkingEnd {
				content_index,
				content: get(event, "content").as_string().unwrap_or_default(),
				partial,
			},
			"toolcall_start" => E::ToolcallStart { content_index, partial },
			"toolcall_delta" => E::ToolcallDelta {
				content_index,
				delta,
				partial,
			},
			"toolcall_end" => E::ToolcallEnd {
				content_index,
				tool_call: crate::json::from_js(get(event, "toolCall")).ok()?,
				partial,
			},
			_ => return None,
		})
	}

	/// Reads block `index` and the message fields other than `content` from the JS partial.
	fn read_block(&self, event: &JsValue, partial: &SharedAssistantMessage, index: usize) {
		let js_partial = get(event, "partial");
		let js_block =
			Reflect::get(&get(&js_partial, "content"), &JsValue::from(index as u32)).unwrap_or(JsValue::UNDEFINED);
		let head = Object::assign(&Object::new(), js_partial.unchecked_ref());
		let _ = Reflect::set(&head, &"content".into(), &js_sys::Array::new());
		let mut message = partial.borrow_mut();
		if let Ok(mut head) = crate::json::from_js::<AssistantMessage>(head.into()) {
			head.content = std::mem::take(&mut message.content);
			*message = head;
		}
		if let Ok(block) = crate::json::from_js::<AssistantContent>(js_block) {
			if index < message.content.len() {
				message.content[index] = block;
			} else {
				message.content.push(block);
			}
		}
	}
}

fn get(target: &JsValue, key: &str) -> JsValue {
	Reflect::get(target, &key.into()).unwrap_or(JsValue::UNDEFINED)
}
