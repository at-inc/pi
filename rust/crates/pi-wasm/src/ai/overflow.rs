//! Exports of `utils/overflow`, and `assistant_facts`: the fields of a JS assistant message these checks read.

use js_sys::{Array, RegExp};
use pi_ai::utils::overflow;
use pi_ai::{AssistantMessage, StopReason, Usage};
use serde::Deserialize;
use wasm_bindgen::prelude::*;

use crate::options::arg;

/// `isContextOverflow(message, contextWindow)`.
#[wasm_bindgen(js_name = aiIsContextOverflow)]
pub fn is_context_overflow(message: JsValue, context_window: Option<f64>) -> Result<bool, JsValue> {
	Ok(overflow::is_context_overflow(
		&assistant_facts(message)?,
		context_window,
	))
}

/// `isRecoverableLength(message, desiredMaxOutput)`.
#[wasm_bindgen(js_name = aiIsRecoverableLength)]
pub fn is_recoverable_length(message: JsValue, desired_max_output: f64) -> Result<bool, JsValue> {
	Ok(overflow::is_recoverable_length(
		&assistant_facts(message)?,
		desired_max_output,
	))
}

/// `getOverflowPatterns()`: new `RegExp` objects of the patterns, as the TS returns copies.
#[wasm_bindgen(js_name = aiGetOverflowPatterns)]
pub fn get_overflow_patterns() -> Array {
	overflow::get_overflow_patterns()
		.into_iter()
		.map(|pattern| JsValue::from(RegExp::new(pattern.source(), "i")))
		.collect()
}

/// The fields of an assistant message that the overflow and retry checks read.
#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Facts {
	stop_reason: Option<String>,
	error_message: Option<String>,
	provider: Option<String>,
	usage: Option<UsageFacts>,
}

#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct UsageFacts {
	#[serde(default)]
	input: f64,
	#[serde(default)]
	output: f64,
	#[serde(default)]
	cache_read: f64,
}

/// An `AssistantMessage` with only the fields the checks read, taken from a JS message; content is not read. A stop
/// reason this version does not know reads as `pending`, which no check matches, as no TS comparison matches it.
pub(super) fn assistant_facts(message: JsValue) -> Result<AssistantMessage, JsValue> {
	let facts: Facts = arg(message)?;
	let usage = facts.usage.unwrap_or_default();
	Ok(AssistantMessage {
		stop_reason: facts
			.stop_reason
			.and_then(|reason| serde_json::from_value(reason.into()).ok())
			.unwrap_or(StopReason::Pending),
		error_message: facts.error_message,
		provider: facts.provider.unwrap_or_default().into(),
		usage: Usage {
			input: usage.input,
			output: usage.output,
			cache_read: usage.cache_read,
			..Usage::default()
		},
		..AssistantMessage::default()
	})
}
