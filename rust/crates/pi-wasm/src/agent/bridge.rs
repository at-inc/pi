//! The JS values of the agent API as Rust values: tools, hooks and stream functions become Rust callbacks over the JS
//! functions, and the contexts the hooks receive become JS objects (snapshots, see `pi_agent::types`).
//!
//! A `Bridge` keeps the JS objects of one run beside the Rust values read from them: tools, messages and models the
//! caller passed in, the messages the run's events created, and the JS signal of the run. A value the run hands back to
//! JS is the caller's object again when it is unchanged, so the identities the TS gives hold: the stream function sees
//! the prompt objects in its context, `message_end` carries the steering message the caller queued, a hook context
//! lists the caller's tools, and every callback of a run receives the same `AbortSignal`.

use std::cell::RefCell;
use std::rc::{Rc, Weak};

use js_sys::{Array, Function, Object};
use pi_agent::{
	AfterToolCallFn, AgentContext, AgentLoopTurnUpdate, AgentMessage, AgentPrepareNextTurnFn,
	AgentPrepareNextTurnWithContextFn, AgentRequestUpdate, AgentTool, AgentToolExecute, AgentTurnContext,
	BeforeToolCallFn, ConvertToLlmFn, FinishTurn, GetApiKeyFn, GetMessagesFn, PrepareArgumentsFn, PrepareNextTurnFn,
	PrepareRequest, StreamFn, TransformContextFn,
};
use pi_ai::{AssistantMessage, Message, Model, OnPayload, OnProviderStreamEvent, OnResponse, Tool};
use pi_host::{AbortSignal, PiError, PiResult};
use serde_json::Value;
use wasm_bindgen::JsCast;
use wasm_bindgen::prelude::*;

use super::{get, optional, set};
use crate::abort::JsSignal;
use crate::options::JsOptions;

type ExecuteFn = <AgentToolExecute as std::ops::Deref>::Target;

/// JS objects behind the Rust values of one agent or loop run.
#[derive(Default)]
pub struct Bridge {
	tools: RefCell<Vec<(Weak<ExecuteFn>, JsValue)>>,
	messages: RefCell<Objects<AgentMessage>>,
	models: RefCell<Objects<Model>>,
	/// The Rust signal of the run, its JS object, and the guard that keeps a JS signal made here following it.
	signal: RefCell<Option<(AbortSignal, JsValue, Option<JsSignal>)>>,
}

impl Bridge {
	/// The Rust tools of a JS `AgentTool[]`.
	pub fn tools(self: &Rc<Self>, tools: &JsValue) -> PiResult<Vec<AgentTool>> {
		let tools: &Array = tools
			.dyn_ref()
			.ok_or_else(|| PiError::type_error("tools must be an array"))?;
		tools.iter().map(|tool| self.tool(tool)).collect()
	}

	/// A JS `AgentTool`: its data fields are the declaration, `execute` and `prepareArguments` are called on the
	/// object (`tool.execute(...)`), read when called.
	fn tool(self: &Rc<Self>, object: JsValue) -> PiResult<AgentTool> {
		let mut fields = JsOptions::new(object.clone());
		let label = fields.take("label").unwrap_or_default();
		let output_schema = fields.take("outputSchema");
		let replay = fields.take("replay");
		let execution_mode = fields.take("executionMode");
		let prepare_arguments = fields
			.function("prepareArguments")
			.map(|function| prepare_arguments(function, object.clone()));
		let mut declaration = fields.rest();
		declaration.entry("parameters").or_insert(Value::Null);
		let tool: Tool = serde_json::from_value(Value::Object(declaration))
			.map_err(|error| PiError::type_error(format!("invalid tool: {error}")))?;
		let execute = execute(object.clone(), Rc::downgrade(self));
		let mut registered = self.tools.borrow_mut();
		registered.retain(|(execute, _)| execute.strong_count() > 0);
		registered.push((Rc::downgrade(&execute), object));
		Ok(AgentTool {
			tool,
			label,
			prepare_arguments,
			output_schema,
			execute,
			replay,
			execution_mode,
		})
	}

	/// The JS objects of `tools`: the caller's object of a tool this bridge converted, else its declaration.
	pub fn tool_objects(&self, tools: &[AgentTool]) -> PiResult<JsValue> {
		let registered = self.tools.borrow();
		let objects = Array::new();
		for tool in tools {
			let known = registered
				.iter()
				.find(|(execute, _)| std::ptr::addr_eq(execute.as_ptr(), Rc::as_ptr(&tool.execute)));
			objects.push(&match known {
				Some((_, object)) => object.clone(),
				None => crate::json::to_js(&tool.tool)?,
			});
		}
		Ok(objects.into())
	}

	/// The Rust messages of a JS `AgentMessage[]`.
	pub fn messages_from_js(&self, messages: &JsValue) -> PiResult<Vec<AgentMessage>> {
		let messages: &Array = messages
			.dyn_ref()
			.ok_or_else(|| PiError::type_error("messages must be an array"))?;
		let mut known = self.messages.borrow_mut();
		messages
			.iter()
			.map(|object| {
				let message: AgentMessage = crate::json::from_js(object.clone())?;
				known.record(message.clone(), object);
				Ok(message)
			})
			.collect()
	}

	/// The Rust messages of a JS `Message[]`.
	fn llm_messages_from_js(&self, messages: &JsValue) -> PiResult<Vec<Message>> {
		self.messages_from_js(messages)?
			.into_iter()
			.map(|message| match message {
				AgentMessage::Llm(message) => Ok(message),
				AgentMessage::Custom(message) => Err(PiError::type_error(format!(
					"not an LLM message: {}",
					message.get("role").unwrap_or(&Value::Null)
				))),
			})
			.collect()
	}

	/// The JS object of `message`: the one it was read from or created as, else a new one.
	pub fn message_to_js(&self, message: &AgentMessage) -> PiResult<JsValue> {
		match self.known_message(message) {
			Some(object) => Ok(object),
			None => crate::json::to_js(message),
		}
	}

	/// The JS object `message` was read from or created as, if any.
	pub fn known_message(&self, message: &AgentMessage) -> Option<JsValue> {
		self.messages.borrow_mut().find(|known| known == message)
	}

	/// Remembers `object` as the JS object of `message`, which the run created.
	pub fn record_message(&self, message: AgentMessage, object: JsValue) {
		self.messages.borrow_mut().entries.push((message, object));
	}

	/// The JS array of `messages`.
	pub fn messages_to_js(&self, messages: &[AgentMessage]) -> PiResult<JsValue> {
		let objects = Array::new();
		for message in messages {
			objects.push(&self.message_to_js(message)?);
		}
		Ok(objects.into())
	}

	/// The JS object of an LLM message.
	fn llm_message_to_js(&self, message: &Message) -> PiResult<JsValue> {
		let known = self
			.messages
			.borrow_mut()
			.find(|known| matches!(known, AgentMessage::Llm(known) if known == message));
		match known {
			Some(object) => Ok(object),
			None => crate::json::to_js(message),
		}
	}

	/// The JS object of an assistant message.
	fn assistant_to_js(&self, message: &AssistantMessage) -> PiResult<JsValue> {
		let known = self
			.messages
			.borrow_mut()
			.find(|known| matches!(known, AgentMessage::Llm(Message::Assistant(known)) if known == message));
		match known {
			Some(object) => Ok(object),
			None => crate::json::to_js(message),
		}
	}

	/// The Rust model of a JS model object.
	pub fn model_from_js(&self, object: JsValue) -> PiResult<Model> {
		let model: Model = crate::json::from_js(object.clone())?;
		self.models.borrow_mut().record(model.clone(), object);
		Ok(model)
	}

	/// The JS object of `model`.
	pub fn model_to_js(&self, model: &Model) -> PiResult<JsValue> {
		match self.models.borrow_mut().find(|known| known == model) {
			Some(object) => Ok(object),
			None => crate::json::to_js(model),
		}
	}

	/// Remembers the objects of an agent's state: `objects` are the JS messages of `messages`, index by index.
	pub fn seed(&self, messages: &[AgentMessage], objects: &JsValue, model: &Model, model_object: JsValue) {
		self.models.borrow_mut().record(model.clone(), model_object);
		let Some(objects) = objects.dyn_ref::<Array>() else {
			return;
		};
		if objects.length() as usize != messages.len() {
			return;
		}
		let mut known = self.messages.borrow_mut();
		known.entries.extend(messages.iter().cloned().zip(objects.iter()));
	}

	/// `{ messages, tools }` of an `AgentContext`.
	pub fn context_to_js(&self, context: &AgentContext) -> PiResult<JsValue> {
		let object = Object::new();
		set(&object, "messages", &self.messages_to_js(&context.messages)?);
		if let Some(tools) = &context.tools {
			set(&object, "tools", &self.tool_objects(tools)?);
		}
		Ok(object.into())
	}

	/// The `AgentContext` of a JS context; its tools become Rust tools.
	pub fn context_from_js(self: &Rc<Self>, value: &JsValue) -> PiResult<AgentContext> {
		let tools = get(value, "tools");
		Ok(AgentContext {
			messages: self.messages_from_js(&get(value, "messages"))?,
			tools: if tools.is_undefined() || tools.is_null() {
				None
			} else {
				Some(self.tools(&tools)?)
			},
		})
	}

	/// The JS signal of `signal`. The bridge keeps the last one, so the calls of one run share it.
	pub fn signal(&self, signal: Option<&AbortSignal>) -> JsValue {
		let Some(signal) = signal else {
			return JsValue::UNDEFINED;
		};
		let mut cached = self.signal.borrow_mut();
		if let Some((known, js, _)) = cached.as_ref()
			&& AbortSignal::ptr_eq(known, signal)
		{
			return js.clone();
		}
		let guard = crate::abort::to_js(signal);
		let value = guard.js.clone();
		*cached = Some((signal.clone(), value.clone(), Some(guard)));
		value
	}

	/// The Rust signal of a caller's JS signal. The callbacks that receive it get the caller's object.
	pub fn signal_from_js(&self, js: &JsValue) -> Option<AbortSignal> {
		let signal = crate::abort::from_js(js)?;
		*self.signal.borrow_mut() = Some((signal.clone(), js.clone(), None));
		Some(signal)
	}

	/// Drops every JS value the bridge holds.
	pub fn release(&self) {
		self.tools.borrow_mut().clear();
		*self.messages.borrow_mut() = Objects::default();
		*self.models.borrow_mut() = Objects::default();
		self.signal.borrow_mut().take();
	}
}

/// JS objects with the Rust values read from them. A search starts after the last match, since a run reads a
/// transcript in order.
struct Objects<T> {
	entries: Vec<(T, JsValue)>,
	next: usize,
}

impl<T> Default for Objects<T> {
	fn default() -> Self {
		Self {
			entries: Vec::new(),
			next: 0,
		}
	}
}

impl<T: PartialEq> Objects<T> {
	fn find(&mut self, matches: impl Fn(&T) -> bool) -> Option<JsValue> {
		let len = self.entries.len();
		let index = (0..len)
			.map(|step| (self.next + step) % len)
			.find(|&index| matches(&self.entries[index].0))?;
		self.next = index + 1;
		Some(self.entries[index].1.clone())
	}

	/// Remembers `object` as the JS object of `value`, once per pair.
	fn record(&mut self, value: T, object: JsValue) {
		if self.find(|known| known == &value).is_some_and(|known| known == object) {
			return;
		}
		self.entries.push((value, object));
	}
}

/// `tool.execute(toolCallId, params, signal, onUpdate)`. `onUpdate` hands each partial result to the loop, which
/// ignores the calls made after the tool settled.
fn execute(tool: JsValue, bridge: Weak<Bridge>) -> AgentToolExecute {
	Rc::new(move |tool_call_id, params, signal, on_update| {
		let tool = tool.clone();
		let (signal, guard) = match bridge.upgrade() {
			Some(bridge) => (bridge.signal(signal.as_ref()), None),
			None => {
				let guard = signal.as_ref().map(crate::abort::to_js);
				(
					guard.as_ref().map_or(JsValue::UNDEFINED, |guard| guard.js.clone()),
					guard,
				)
			}
		};
		Box::pin(async move {
			let _guard = guard;
			let execute: Function = get(&tool, "execute")
				.dyn_into()
				.map_err(|_| PiError::type_error("tool.execute is not a function"))?;
			let on_update = match on_update {
				Some(on_update) => Closure::<dyn Fn(JsValue)>::new(move |partial: JsValue| {
					if let Ok(partial) = crate::json::from_js(partial) {
						on_update(partial);
					}
				})
				.into_js_value(),
				None => JsValue::UNDEFINED,
			};
			let args = [tool_call_id.into(), crate::json::to_js(&params)?, signal, on_update];
			let result = crate::callback::call(&execute, &tool, &args).await?;
			crate::json::from_js(result)
		})
	})
}

/// `tool.prepareArguments(args)`, called synchronously as in the TS.
fn prepare_arguments(function: Function, tool: JsValue) -> PrepareArgumentsFn {
	Rc::new(move |args| {
		let args = crate::json::to_js(&args)?;
		let result = function.call1(&tool, &args).map_err(crate::error::from_js)?;
		crate::json::from_js(result)
	})
}

/// `await function(...args)`.
async fn call(function: &Function, args: &[JsValue]) -> PiResult<JsValue> {
	crate::callback::call(function, &JsValue::UNDEFINED, args).await
}

/// A JS `StreamFn`: called with the model, the transcript and the options as JS values, the run's objects where
/// `bridge` knows them; the stream it returns (or resolves to) is read with `events::from_js`.
pub fn stream_fn(function: Function, bridge: Option<Rc<Bridge>>) -> StreamFn {
	Rc::new(move |model, context, options| {
		let function = function.clone();
		let args = (|| -> PiResult<_> {
			let (options, signal) = match &options {
				Some(options) => crate::options::simple_options_to_js(options)?,
				None => (JsValue::UNDEFINED, None),
			};
			let Some(bridge) = &bridge else {
				return Ok((
					[crate::json::to_js(model)?, crate::json::to_js(context)?, options],
					signal,
				));
			};
			let messages = Array::new();
			for message in &context.messages {
				messages.push(&bridge.llm_message_to_js(message)?);
			}
			let transcript = Object::new();
			set(&transcript, "messages", &messages);
			Ok(([bridge.model_to_js(model)?, transcript.into(), options], signal))
		})();
		Box::pin(async move {
			let (args, signal) = args?;
			let stream = call(&function, &args).await?;
			Ok(crate::events::from_js(stream, signal))
		})
	})
}

/// `convertToLlm(messages)`.
pub fn convert_to_llm(bridge: Rc<Bridge>, function: Function) -> ConvertToLlmFn {
	Rc::new(move |messages| {
		let (bridge, function) = (bridge.clone(), function.clone());
		Box::pin(async move {
			let messages = bridge.messages_to_js(&messages)?;
			bridge.llm_messages_from_js(&call(&function, &[messages]).await?)
		})
	})
}

/// `transformContext(messages, signal)`.
pub fn transform_context(bridge: Rc<Bridge>, function: Function) -> TransformContextFn {
	Rc::new(move |messages, signal| {
		let (bridge, function) = (bridge.clone(), function.clone());
		let signal = bridge.signal(signal.as_ref());
		Box::pin(async move {
			let messages = bridge.messages_to_js(&messages)?;
			bridge.messages_from_js(&call(&function, &[messages, signal]).await?)
		})
	})
}

/// `getApiKey(provider)`.
pub fn get_api_key(function: Function) -> GetApiKeyFn {
	Rc::new(move |provider| {
		let function = function.clone();
		Box::pin(async move { optional(call(&function, &[provider.into()]).await?) })
	})
}

/// `getSteeringMessages()` and `getFollowUpMessages()`; `undefined` reads as no messages.
pub fn get_messages(bridge: Rc<Bridge>, function: Function) -> GetMessagesFn {
	Rc::new(move || {
		let (bridge, function) = (bridge.clone(), function.clone());
		Box::pin(async move {
			let messages = call(&function, &[]).await?;
			if messages.is_undefined() || messages.is_null() {
				return Ok(Vec::new());
			}
			bridge.messages_from_js(&messages)
		})
	})
}

/// `beforeToolCall(context, signal)`. The hook's edits of `context.args` (in place or by assignment) reach the
/// execution, like the shared arguments of the Rust context.
pub fn before_tool_call(bridge: Rc<Bridge>, function: Function) -> BeforeToolCallFn {
	Rc::new(move |context, signal| {
		let (bridge, function) = (bridge.clone(), function.clone());
		let signal = bridge.signal(signal.as_ref());
		Box::pin(async move {
			let object = Object::new();
			set(
				&object,
				"assistantMessage",
				&bridge.assistant_to_js(&context.assistant_message)?,
			);
			set(&object, "toolCall", &crate::json::to_js(&context.tool_call)?);
			set(&object, "args", &crate::json::to_js(&*context.args.borrow())?);
			set(&object, "context", &bridge.context_to_js(&context.context)?);
			let result = call(&function, &[object.clone().into(), signal]).await?;
			*context.args.borrow_mut() = crate::json::from_js(get(&object, "args"))?;
			optional(result)
		})
	})
}

/// `afterToolCall(context, signal)`.
pub fn after_tool_call(bridge: Rc<Bridge>, function: Function) -> AfterToolCallFn {
	Rc::new(move |context, signal| {
		let (bridge, function) = (bridge.clone(), function.clone());
		let signal = bridge.signal(signal.as_ref());
		Box::pin(async move {
			let object = Object::new();
			set(
				&object,
				"assistantMessage",
				&bridge.assistant_to_js(&context.assistant_message)?,
			);
			set(&object, "toolCall", &crate::json::to_js(&context.tool_call)?);
			set(&object, "args", &crate::json::to_js(&context.args)?);
			set(&object, "result", &crate::json::to_js(&context.result)?);
			set(&object, "isError", &context.is_error.into());
			set(&object, "context", &bridge.context_to_js(&context.context)?);
			optional(call(&function, &[object.into(), signal]).await?)
		})
	})
}

/// `{ message, toolResults, context, newMessages }` of an `AgentTurnContext`.
fn turn_context(bridge: &Bridge, context: &AgentTurnContext) -> PiResult<JsValue> {
	let object = Object::new();
	let tool_results = Array::new();
	for result in &context.tool_results {
		tool_results.push(&bridge.llm_message_to_js(&Message::ToolResult(result.clone()))?);
	}
	set(&object, "message", &bridge.assistant_to_js(&context.message)?);
	set(&object, "toolResults", &tool_results);
	set(&object, "context", &bridge.context_to_js(&context.context)?);
	set(&object, "newMessages", &bridge.messages_to_js(&context.new_messages)?);
	Ok(object.into())
}

/// `finishTurn(context, signal)`.
pub fn finish_turn(bridge: Rc<Bridge>, function: Function) -> FinishTurn {
	Rc::new(move |context, signal| {
		let (bridge, function) = (bridge.clone(), function.clone());
		let signal = bridge.signal(signal.as_ref());
		Box::pin(async move { optional(call(&function, &[turn_context(&bridge, &context)?, signal]).await?) })
	})
}

/// `prepareRequest({ context, model, thinkingLevel }, signal)`.
pub fn prepare_request(bridge: Rc<Bridge>, function: Function) -> PrepareRequest {
	Rc::new(move |request, signal| {
		let (bridge, function) = (bridge.clone(), function.clone());
		let signal = bridge.signal(signal.as_ref());
		Box::pin(async move {
			let object = Object::new();
			set(&object, "context", &bridge.context_to_js(&request.context)?);
			set(&object, "model", &bridge.model_to_js(&request.model)?);
			set(&object, "thinkingLevel", &crate::json::to_js(&request.thinking_level)?);
			let update = call(&function, &[object.into(), signal]).await?;
			let Some(update) = turn_update(&bridge, update)? else {
				return Ok(None);
			};
			Ok(Some(AgentRequestUpdate {
				context: update.context,
				model: update.model,
				thinking_level: update.thinking_level,
			}))
		})
	})
}

/// `prepareNextTurn(context)` of `AgentLoopConfig`.
pub fn prepare_next_turn(bridge: Rc<Bridge>, function: Function) -> PrepareNextTurnFn {
	Rc::new(move |context| {
		let (bridge, function) = (bridge.clone(), function.clone());
		Box::pin(async move { turn_update(&bridge, call(&function, &[turn_context(&bridge, &context)?]).await?) })
	})
}

/// `prepareNextTurn(signal)` of `AgentOptions`.
pub fn agent_prepare_next_turn(bridge: Rc<Bridge>, function: Function) -> AgentPrepareNextTurnFn {
	Rc::new(move |signal| {
		let (bridge, function) = (bridge.clone(), function.clone());
		let signal = bridge.signal(signal.as_ref());
		Box::pin(async move { turn_update(&bridge, call(&function, &[signal]).await?) })
	})
}

/// `prepareNextTurnWithContext(context, signal)` of `AgentOptions`.
pub fn agent_prepare_next_turn_with_context(
	bridge: Rc<Bridge>,
	function: Function,
) -> AgentPrepareNextTurnWithContextFn {
	Rc::new(move |context, signal| {
		let (bridge, function) = (bridge.clone(), function.clone());
		let signal = bridge.signal(signal.as_ref());
		Box::pin(async move {
			turn_update(
				&bridge,
				call(&function, &[turn_context(&bridge, &context)?, signal]).await?,
			)
		})
	})
}

/// An `AgentLoopTurnUpdate` of a JS value, `None` for `undefined`/`null`.
fn turn_update(bridge: &Rc<Bridge>, value: JsValue) -> PiResult<Option<AgentLoopTurnUpdate>> {
	if value.is_undefined() || value.is_null() {
		return Ok(None);
	}
	let context = get(&value, "context");
	Ok(Some(AgentLoopTurnUpdate {
		context: if context.is_undefined() || context.is_null() {
			None
		} else {
			Some(bridge.context_from_js(&context)?)
		},
		messages: match get(&value, "messages") {
			messages if messages.is_undefined() || messages.is_null() => None,
			messages => Some(bridge.messages_from_js(&messages)?),
		},
		model: match get(&value, "model") {
			model if model.is_undefined() || model.is_null() => None,
			model => Some(bridge.model_from_js(model)?),
		},
		thinking_level: optional(get(&value, "thinkingLevel"))?,
	}))
}

/// `onPayload(payload, model)`. A callback that returns `undefined` keeps the payload with its in-place edits.
pub fn on_payload(function: Function) -> OnPayload {
	Rc::new(move |payload, model| {
		let function = function.clone();
		let args = crate::json::to_js(&payload).and_then(|payload| Ok((payload, crate::json::to_js(model)?)));
		Box::pin(async move {
			let (payload, model) = args?;
			let result = call(&function, &[payload.clone(), model]).await?;
			let replacement = if result.is_undefined() { payload } else { result };
			Ok(Some(crate::json::from_js(replacement)?))
		})
	})
}

/// `onResponse({ status, headers }, model)`.
pub fn on_response(function: Function) -> OnResponse {
	Rc::new(move |response, model| {
		let function = function.clone();
		let args = (|| -> PiResult<_> {
			let object = Object::new();
			set(&object, "status", &JsValue::from(response.status));
			set(&object, "headers", &crate::json::to_js(&response.headers)?);
			Ok([object.into(), crate::json::to_js(model)?])
		})();
		Box::pin(async move {
			call(&function, &args?).await?;
			Ok(())
		})
	})
}

/// `onProviderStreamEvent(event, model)`.
pub fn on_provider_stream_event(function: Function) -> OnProviderStreamEvent {
	Rc::new(move |event, model| {
		let function = function.clone();
		let args = crate::json::to_js(event).and_then(|event| Ok([event, crate::json::to_js(model)?]));
		Box::pin(async move {
			call(&function, &args?).await?;
			Ok(())
		})
	})
}
