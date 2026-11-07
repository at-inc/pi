//! Exports behind `@at-inc/pi-ai-rs`, one file per facade module of `rust/npm/pi-ai/src`. Export names are `ai` plus
//! the TS function name; handles are classes named after the TS object they stand for.

mod anthropic_messages;
mod azure_openai_responses;
mod bedrock_converse_stream;
mod compat;
mod github_copilot_headers;
mod google_generative_ai;
mod google_shared;
mod google_vertex;
mod images;
mod lazy;
mod mistral_conversations;
mod models;
mod models_store;
mod openai_codex_responses;
mod openai_completions;
mod openai_responses;
mod pi_messages;
mod provider_bridge;
mod providers;
mod simple_options;
mod stores;
mod transcript;
mod transform_messages;

use pi_ai::{Context, Model, TranscriptContext};
use wasm_bindgen::JsValue;

use crate::options::arg;

/// The `Model` of a JS model object.
fn model(value: &JsValue) -> Result<Model, JsValue> {
	arg(value.clone())
}

/// A model at a `Models` entry point that takes one model type. The TS checks the type at run time and reports a model of
/// another type as an error result; such a model lacks fields of this type, so it is read with `fill` supplying them,
/// and the core's own type check rejects it as the TS does.
fn entry_model<T: serde::de::DeserializeOwned>(
	value: &JsValue,
	fill: &[(&str, fn() -> serde_json::Value)],
) -> Result<T, JsValue> {
	let error = match arg::<T>(value.clone()) {
		Ok(model) => return Ok(model),
		Err(error) => error,
	};
	let Ok(serde_json::Value::Object(mut object)) = crate::json::from_js(value.clone()) else {
		return Err(error);
	};
	for (key, default) in fill {
		object.entry(*key).or_insert_with(default);
	}
	serde_json::from_value(serde_json::Value::Object(object)).map_err(|_| error)
}

/// The `TranscriptContext` of a JS transcript. A transcript is a `Context` with only `messages`, which
/// `normalizeContext` leaves unchanged, so a JS `Context` is accepted too.
fn transcript(value: JsValue) -> Result<TranscriptContext, JsValue> {
	let context: Context = arg(value)?;
	Ok(pi_ai::normalize_context(&context))
}
