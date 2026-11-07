//! Exports behind `@at-inc/pi-ai-rs`, one file per facade module of `rust/npm/pi-ai/src`. Export names are `ai` plus
//! the TS function name; handles are classes named after the TS object they stand for.

mod auth;
mod auth_context;
mod auth_helpers;
mod auth_resolve;
mod bun_oauth;
mod callback_server;
mod compat;
mod credential_store;
mod device_code;
mod models;
mod models_store;
mod oauth_flows;
mod oauth_load;
mod oauth_page;
mod openai_completions;
mod pkce;
mod transcript;
mod trusted_url;

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
