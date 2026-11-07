//! Exports of `compat`: the api-dispatch `stream`/`streamSimple` with env API keys and the builtin catalog reads.

use wasm_bindgen::prelude::*;

use crate::events::EventSink;
use crate::options::{arg, provider_stream_options, simple_stream_options, throw};

/// `stream(model, context, options)`, sending the events to `sink`.
#[wasm_bindgen(js_name = aiStream)]
pub fn stream(model: JsValue, context: JsValue, options: JsValue, sink: EventSink) -> Result<(), JsValue> {
	let parsed = super::model(&model)?;
	let context: pi_ai::Context = arg(context)?;
	let options = provider_stream_options(options, &model);
	crate::events::send(throw(pi_ai::compat::stream(&parsed, &context, options))?, sink);
	Ok(())
}

/// `streamSimple(model, context, options)`, sending the events to `sink`.
#[wasm_bindgen(js_name = aiStreamSimple)]
pub fn stream_simple(model: JsValue, context: JsValue, options: JsValue, sink: EventSink) -> Result<(), JsValue> {
	let parsed = super::model(&model)?;
	let context: pi_ai::Context = arg(context)?;
	let options = simple_stream_options(options, &model);
	crate::events::send(throw(pi_ai::compat::stream_simple(&parsed, &context, options))?, sink);
	Ok(())
}

/// `getModel(provider, modelId)` (`getBuiltinModel`): a builtin catalog model, `undefined` when unknown.
#[wasm_bindgen(js_name = aiGetBuiltinModel)]
pub fn get_builtin_model(provider: &str, model_id: &str) -> Result<JsValue, JsValue> {
	match pi_ai::providers::all::get_builtin_model(provider, model_id) {
		Some(model) => throw(crate::json::to_js(&model)),
		None => Ok(JsValue::UNDEFINED),
	}
}
