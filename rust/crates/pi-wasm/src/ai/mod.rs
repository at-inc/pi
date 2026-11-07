//! Exports behind `@at-inc/pi-ai-rs`, one file per facade module of `rust/npm/pi-ai/src`. Export names are `ai` plus
//! the TS function name; handles are classes named after the TS object they stand for.

mod anthropic_messages;
mod azure_openai_responses;
mod bedrock_converse_stream;
mod compat;
mod github_copilot_headers;
mod google_generative_ai;
mod google_vertex;
mod mistral_conversations;
mod models;
mod openai_codex_responses;
mod openai_completions;
mod openai_responses;
mod pi_messages;
mod provider_bridge;
mod simple_options;
mod transcript;
mod transform_messages;

use pi_ai::{Context, Model, TranscriptContext};
use wasm_bindgen::JsValue;

use crate::options::arg;

/// The `Model` of a JS model object.
fn model(value: &JsValue) -> Result<Model, JsValue> {
	arg(value.clone())
}

/// The `TranscriptContext` of a JS transcript. A transcript is a `Context` with only `messages`, which
/// `normalizeContext` leaves unchanged, so a JS `Context` is accepted too.
fn transcript(value: JsValue) -> Result<TranscriptContext, JsValue> {
	let context: Context = arg(value)?;
	Ok(pi_ai::normalize_context(&context))
}
