//! Exports of `agent-loop`: `runAgentLoop` and `runAgentLoopContinue` over a JS context and config, `runToolCall`,
//! and `DEFAULT_AGENT_IDENTITY`. The facade builds `agentLoop` and `agentLoopContinue` on the first two, as the TS
//! does. A loop's events reach the caller's `emit` through an `AgentEventSink`, as the agent's do.

use std::rc::Rc;

use js_sys::{Object, Promise};
use pi_agent::{AgentContext, AgentLoopConfig, AgentMessage, RunToolCallOptions, StreamFn, ToolCallHooks};
use pi_ai::{Message, SimpleStreamOptions};
use pi_host::{AbortSignal, LocalBoxFuture, PiError, PiResult};
use serde_json::Value;
use wasm_bindgen::prelude::*;

use super::bridge::{self, Bridge};
use super::events::{AgentEventSink, emitter, settle};
use super::{function, get, set};
use crate::options::{JsOptions, throw};

/// `runAgentLoop(prompts, context, config, emit, signal, streamFn)`, with `emit` behind `sink`.
#[wasm_bindgen(js_name = agentRunAgentLoop)]
pub fn run_agent_loop(
	prompts: JsValue,
	context: JsValue,
	config: JsValue,
	sink: AgentEventSink,
	signal: JsValue,
	stream_fn: JsValue,
) -> Promise {
	let bridge = Rc::new(Bridge::default());
	let args = (|| -> PiResult<_> {
		let prompts = bridge.messages_from_js(&prompts)?;
		Ok((prompts, loop_args(&bridge, &context, &config, &signal, stream_fn)?))
	})();
	settled(bridge.clone(), async move {
		let (prompts, (context, config, signal, stream_fn)) = args?;
		pi_agent::run_agent_loop(prompts, context, config, emitter(sink, bridge), signal, stream_fn).await
	})
}

/// `runAgentLoopContinue(context, config, emit, signal, streamFn)`, with `emit` behind `sink`.
#[wasm_bindgen(js_name = agentRunAgentLoopContinue)]
pub fn run_agent_loop_continue(
	context: JsValue,
	config: JsValue,
	sink: AgentEventSink,
	signal: JsValue,
	stream_fn: JsValue,
) -> Promise {
	let bridge = Rc::new(Bridge::default());
	let args = loop_args(&bridge, &context, &config, &signal, stream_fn);
	settled(bridge.clone(), async move {
		let (context, config, signal, stream_fn) = args?;
		pi_agent::run_agent_loop_continue(context, config, emitter(sink, bridge), signal, stream_fn).await
	})
}

type LoopArgs = (AgentContext, AgentLoopConfig, Option<AbortSignal>, Option<StreamFn>);

fn loop_args(
	bridge: &Rc<Bridge>,
	context: &JsValue,
	config: &JsValue,
	signal: &JsValue,
	stream_fn: JsValue,
) -> PiResult<LoopArgs> {
	Ok((
		bridge.context_from_js(context)?,
		loop_config(bridge, config)?,
		bridge.signal_from_js(signal),
		stream_fn
			.dyn_into()
			.ok()
			.map(|f| bridge::stream_fn(f, Some(bridge.clone()))),
	))
}

/// The messages of a finished loop as a promise, as the objects the listeners received; the run's JS values are
/// dropped when it settles.
fn settled(bridge: Rc<Bridge>, run: impl Future<Output = PiResult<Vec<AgentMessage>>> + 'static) -> Promise {
	wasm_bindgen_futures::future_to_promise(async move {
		let result = run.await.and_then(|messages| bridge.messages_to_js(&messages));
		bridge.release();
		result.map_err(|error| crate::error::to_js(&error))
	})
}

/// The Rust config of a JS `AgentLoopConfig`: the callbacks, `model` and `toolExecution`, and the stream options,
/// whose callbacks receive the caller's model object.
fn loop_config(bridge: &Rc<Bridge>, config: &JsValue) -> PiResult<AgentLoopConfig> {
	let model_object = get(config, "model");
	let model = bridge.model_from_js(model_object.clone())?;
	let mut options = JsOptions::new(config.clone());
	options.take::<Value>("model");
	let tool_execution = options.take("toolExecution");
	let convert_to_llm = match options.function("convertToLlm") {
		Some(convert_to_llm) => bridge::convert_to_llm(bridge.clone(), convert_to_llm),
		None => Rc::new(missing_convert_to_llm),
	};
	let mut loop_config = AgentLoopConfig::new(model, convert_to_llm);
	loop_config.transform_context = options
		.function("transformContext")
		.map(|f| bridge::transform_context(bridge.clone(), f));
	loop_config.get_api_key = options.function("getApiKey").map(bridge::get_api_key);
	loop_config.finish_turn = options
		.function("finishTurn")
		.map(|f| bridge::finish_turn(bridge.clone(), f));
	loop_config.prepare_request = options
		.function("prepareRequest")
		.map(|f| bridge::prepare_request(bridge.clone(), f));
	loop_config.prepare_next_turn = options
		.function("prepareNextTurn")
		.map(|f| bridge::prepare_next_turn(bridge.clone(), f));
	loop_config.get_steering_messages = options
		.function("getSteeringMessages")
		.map(|f| bridge::get_messages(bridge.clone(), f));
	loop_config.get_follow_up_messages = options
		.function("getFollowUpMessages")
		.map(|f| bridge::get_messages(bridge.clone(), f));
	loop_config.before_tool_call = options
		.function("beforeToolCall")
		.map(|f| bridge::before_tool_call(bridge.clone(), f));
	loop_config.after_tool_call = options
		.function("afterToolCall")
		.map(|f| bridge::after_tool_call(bridge.clone(), f));
	loop_config.tool_execution = tool_execution;
	let base = options.stream_options(&model_object);
	loop_config.base = SimpleStreamOptions {
		base,
		tool_choice: options.take("toolChoice"),
		reasoning: options.take("reasoning"),
		deferred: options.take("deferred"),
		thinking_budgets: options.take("thinkingBudgets"),
		extra: Default::default(),
	};
	loop_config.base.extra = options.rest();
	Ok(loop_config)
}

/// A config without `convertToLlm` fails its first request, as the TS call does.
fn missing_convert_to_llm(_: Vec<AgentMessage>) -> LocalBoxFuture<'static, PiResult<Vec<Message>>> {
	Box::pin(async { Err(PiError::type_error("config.convertToLlm is not a function")) })
}

/// `runToolCall(toolCall, options)`. The outcome's `toolCall` is the caller's object, as in the TS.
#[wasm_bindgen(js_name = agentRunToolCall)]
pub fn run_tool_call(tool_call: JsValue, options: JsValue) -> Promise {
	let bridge = Rc::new(Bridge::default());
	let args = (|| -> PiResult<_> {
		let call = crate::json::from_js(tool_call.clone())?;
		let hooks = ToolCallHooks {
			before_tool_call: function(&options, "beforeToolCall").map(|f| bridge::before_tool_call(bridge.clone(), f)),
			after_tool_call: function(&options, "afterToolCall").map(|f| bridge::after_tool_call(bridge.clone(), f)),
		};
		let on_update = function(&options, "onUpdate").map(|on_update| -> pi_agent::ToolUpdateSink {
			Rc::new(move |partial| {
				let pending = crate::json::to_js(&partial).and_then(|partial| {
					on_update
						.call1(&JsValue::UNDEFINED, &partial)
						.map_err(crate::error::from_js)
				});
				settle(pending)
			})
		});
		let options = RunToolCallOptions {
			base: hooks,
			tools: bridge.tools(&get(&options, "tools"))?,
			assistant_message: crate::json::from_js(get(&options, "assistantMessage"))?,
			context: bridge.context_from_js(&get(&options, "context"))?,
			signal: bridge.signal_from_js(&get(&options, "signal")),
			on_update,
		};
		Ok((call, options))
	})();
	wasm_bindgen_futures::future_to_promise(async move {
		let result = async {
			let (call, options) = args?;
			let outcome = pi_agent::run_tool_call(call, options).await?;
			let object = Object::new();
			set(&object, "toolCall", &tool_call);
			set(&object, "result", &crate::json::to_js(&outcome.result)?);
			set(&object, "isError", &outcome.is_error.into());
			if let Some(duration_ms) = outcome.duration_ms {
				set(&object, "durationMs", &duration_ms.into());
			}
			Ok(JsValue::from(object))
		}
		.await;
		bridge.release();
		result.map_err(|error: PiError| crate::error::to_js(&error))
	})
}

/// `DEFAULT_AGENT_IDENTITY`.
#[wasm_bindgen(js_name = agentDefaultAgentIdentity)]
pub fn default_agent_identity() -> Result<JsValue, JsValue> {
	throw(crate::json::to_js(&*pi_agent::DEFAULT_AGENT_IDENTITY))
}
