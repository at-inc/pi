//! Exports of `api/openai-completions` and `api/openai-completions.lazy`.

use std::rc::Rc;

use pi_ai::Model;
use pi_ai::api::openai_completions::{self, OpenAICompletionsOptions};
use serde_json::{Map, Value};
use wasm_bindgen::prelude::*;

use super::models::NativeStreams;
use crate::events::EventSink;
use crate::options::{arg, provider_stream_options, simple_stream_options, throw};

/// `stream(model, context, options)` of the module, sending the events to `sink`.
#[wasm_bindgen(js_name = aiOpenAICompletionsStream)]
pub fn stream(model: JsValue, context: JsValue, options: JsValue, sink: EventSink) -> Result<(), JsValue> {
	let parsed = super::model(&model)?;
	let context = super::transcript(context)?;
	let options = provider_stream_options(options, &model).map(OpenAICompletionsOptions::from);
	crate::events::send(openai_completions::stream(&parsed, &context, options), sink);
	Ok(())
}

/// `streamSimple(model, context, options)` of the module, sending the events to `sink`.
#[wasm_bindgen(js_name = aiOpenAICompletionsStreamSimple)]
pub fn stream_simple(model: JsValue, context: JsValue, options: JsValue, sink: EventSink) -> Result<(), JsValue> {
	let parsed = super::model(&model)?;
	let context = super::transcript(context)?;
	let options = simple_stream_options(options, &model);
	crate::events::send(
		throw(openai_completions::stream_simple(&parsed, &context, options))?,
		sink,
	);
	Ok(())
}

/// `convertMessages(model, context, compat)`. `compat` defaults to the model's resolved compat; a given one is
/// resolved over the model's like `model.compat`, so a complete object is used as is.
#[wasm_bindgen(js_name = aiOpenAICompletionsConvertMessages)]
pub fn convert_messages(model: JsValue, context: JsValue, compat: JsValue) -> Result<JsValue, JsValue> {
	let mut parsed: Model = super::model(&model)?;
	let context = super::transcript(context)?;
	if !compat.is_undefined() && !compat.is_null() {
		parsed.compat = Some(arg::<Map<String, Value>>(compat)?);
	}
	let compat = openai_completions::get_compat(&parsed);
	let messages = throw(openai_completions::convert_messages(&parsed, &context, &compat, None))?;
	throw(crate::json::to_js(&messages))
}

/// `openAICompletionsApi()`: the module as `ProviderStreams`.
#[wasm_bindgen(js_name = aiOpenAICompletionsApi)]
pub fn openai_completions_api() -> NativeStreams {
	NativeStreams::new(Rc::new(openai_completions::OpenAICompletionsStreams))
}
