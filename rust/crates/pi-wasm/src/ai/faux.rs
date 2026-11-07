//! Exports of `providers/faux` and compat's `registerFauxProvider`: the faux core behind a handle, with JS response
//! factories bridged as Rust ones.

use std::rc::Rc;

use js_sys::{Array, Function};
use pi_ai::{
	AssistantMessage, FauxAssistantContent, FauxAssistantMessageOptions, FauxCore, FauxResponseFactory,
	FauxResponseStep, FauxToolCallOptions, JsonObject, RegisterFauxProviderOptions,
};
use pi_host::PiResult;
use wasm_bindgen::JsCast;
use wasm_bindgen::prelude::*;

use super::models::ProviderHandle;
use crate::options::{arg, throw};

/// `fauxText(text)`.
#[wasm_bindgen(js_name = aiFauxText)]
pub fn faux_text(text: &str) -> Result<JsValue, JsValue> {
	throw(crate::json::to_js(&pi_ai::faux_text(text)))
}

/// `fauxThinking(thinking)`.
#[wasm_bindgen(js_name = aiFauxThinking)]
pub fn faux_thinking(thinking: &str) -> Result<JsValue, JsValue> {
	throw(crate::json::to_js(&pi_ai::faux_thinking(thinking)))
}

/// `fauxToolCall(name, arguments, options)`.
#[wasm_bindgen(js_name = aiFauxToolCall)]
pub fn faux_tool_call(name: &str, arguments: JsValue, options: JsValue) -> Result<JsValue, JsValue> {
	let arguments: JsonObject = arg(arguments)?;
	let options: Option<FauxToolCallOptions> = arg(options)?;
	throw(crate::json::to_js(&pi_ai::faux_tool_call(name, arguments, options)))
}

/// `fauxAssistantMessage(content, options)`.
#[wasm_bindgen(js_name = aiFauxAssistantMessage)]
pub fn faux_assistant_message(content: JsValue, options: JsValue) -> Result<JsValue, JsValue> {
	let content = if let Some(text) = content.as_string() {
		FauxAssistantContent::Text(text)
	} else if content.is_array() {
		FauxAssistantContent::Blocks(arg(content)?)
	} else {
		FauxAssistantContent::Block(arg(content)?)
	};
	let options: Option<FauxAssistantMessageOptions> = arg(options)?;
	throw(crate::json::to_js(&pi_ai::faux_assistant_message(content, options)))
}

/// `createFauxCore(options)`'s result, shared by `fauxProvider()` and `registerFauxProvider()`.
#[wasm_bindgen]
pub struct FauxCoreHandle {
	core: FauxCore,
	provider: Option<ProviderHandle>,
	unregister: Option<Rc<dyn Fn()>>,
}

#[wasm_bindgen]
impl FauxCoreHandle {
	#[wasm_bindgen(getter)]
	pub fn api(&self) -> String {
		self.core.api.clone()
	}

	/// The models, as JS objects.
	pub fn models(&self) -> Result<JsValue, JsValue> {
		throw(crate::json::to_js(&self.core.models))
	}

	/// `state.callCount`.
	#[wasm_bindgen(js_name = callCount)]
	pub fn call_count(&self) -> usize {
		self.core.state.borrow().call_count
	}

	/// `state.deferredFetchCount`.
	#[wasm_bindgen(js_name = deferredFetchCount)]
	pub fn deferred_fetch_count(&self) -> usize {
		self.core.state.borrow().deferred_fetch_count
	}

	/// `state.cancelledDeferred`, as new JS objects.
	#[wasm_bindgen(js_name = cancelledDeferred)]
	pub fn cancelled_deferred(&self) -> Result<JsValue, JsValue> {
		throw(crate::json::to_js(&self.core.state.borrow().cancelled_deferred))
	}

	/// `setResponses(responses)`; a function step is called as `step(context, options, model)`.
	#[wasm_bindgen(js_name = setResponses)]
	pub fn set_responses(&self, responses: Array) -> Result<(), JsValue> {
		self.core.set_responses(steps(responses)?);
		Ok(())
	}

	/// `appendResponses(responses)`.
	#[wasm_bindgen(js_name = appendResponses)]
	pub fn append_responses(&self, responses: Array) -> Result<(), JsValue> {
		self.core.append_responses(steps(responses)?);
		Ok(())
	}

	/// `getPendingResponseCount()`.
	#[wasm_bindgen(js_name = getPendingResponseCount)]
	pub fn get_pending_response_count(&self) -> usize {
		self.core.get_pending_response_count()
	}

	/// `fauxProvider()`'s `provider`.
	pub fn provider(&self) -> Option<ProviderHandle> {
		self.provider.as_ref().map(ProviderHandle::share)
	}

	/// `registerFauxProvider()`'s `unregister()`.
	pub fn unregister(&self) {
		if let Some(unregister) = &self.unregister {
			unregister();
		}
	}
}

/// `fauxProvider(options)`.
#[wasm_bindgen(js_name = aiFauxProvider)]
pub fn faux_provider(options: JsValue) -> Result<FauxCoreHandle, JsValue> {
	let handle = pi_ai::faux_provider(arg::<Option<RegisterFauxProviderOptions>>(options)?);
	Ok(FauxCoreHandle {
		core: handle.core,
		provider: Some(ProviderHandle::from_provider(handle.provider)),
		unregister: None,
	})
}

/// `registerFauxProvider(options)`.
#[wasm_bindgen(js_name = aiRegisterFauxProvider)]
pub fn register_faux_provider(options: JsValue) -> Result<FauxCoreHandle, JsValue> {
	let registration = pi_ai::compat::register_faux_provider(arg::<Option<RegisterFauxProviderOptions>>(options)?);
	Ok(FauxCoreHandle {
		core: registration.core,
		provider: None,
		unregister: Some(registration.unregister),
	})
}

/// The Rust steps of a JS `FauxResponseStep[]`.
fn steps(responses: Array) -> Result<Vec<FauxResponseStep>, JsValue> {
	responses
		.iter()
		.map(|step| match step.dyn_into::<Function>() {
			Ok(function) => Ok(FauxResponseStep::Factory(factory(function))),
			Err(message) => arg::<AssistantMessage>(message).map(FauxResponseStep::Message),
		})
		.collect()
}

/// A JS factory as a Rust one. The facade binds its live `state` view into the function, so the Rust state is not
/// passed.
fn factory(function: Function) -> FauxResponseFactory {
	Rc::new(move |context, options, _state, model| {
		let function = function.clone();
		let inputs = (|| -> PiResult<_> {
			let options = options.map(crate::options::simple_options_to_js).transpose()?;
			Ok((crate::json::to_js(context)?, options, crate::json::to_js(model)?))
		})();
		Box::pin(async move {
			let (context, options, model) = inputs?;
			let (options, _signal) = options.unwrap_or((JsValue::UNDEFINED, None));
			let message = crate::callback::call(&function, &JsValue::UNDEFINED, &[context, options, model]).await?;
			crate::json::from_js::<AssistantMessage>(message)
		})
	})
}
