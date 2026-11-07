//! Exports of `utils/estimate`. Messages cross as JSON text (`json::from_js_text`).

use js_sys::Reflect;
use pi_ai::utils::estimate;
use pi_ai::{Message, Usage, UserMessageContent};
use wasm_bindgen::prelude::*;

use crate::json::{from_js_text, to_js};
use crate::options::throw;

/// `calculateContextTokens(usage)`.
#[wasm_bindgen(js_name = aiCalculateContextTokens)]
pub fn calculate_context_tokens(usage: JsValue) -> Result<f64, JsValue> {
	let usage: Usage = throw(from_js_text(&usage))?;
	Ok(estimate::calculate_context_tokens(&usage))
}

/// `estimateTextTokens(text)`.
#[wasm_bindgen(js_name = aiEstimateTextTokens)]
pub fn estimate_text_tokens(text: &str) -> f64 {
	estimate::estimate_text_tokens(text)
}

/// `estimateTextAndImageContentTokens(content)`: a string or text and image blocks.
#[wasm_bindgen(js_name = aiEstimateTextAndImageContentTokens)]
pub fn estimate_text_and_image_content_tokens(content: JsValue) -> Result<f64, JsValue> {
	let content: UserMessageContent = throw(from_js_text(&content))?;
	Ok(estimate::estimate_text_and_image_content_tokens(&content))
}

/// `estimateMessageTokens(message)`.
#[wasm_bindgen(js_name = aiEstimateMessageTokens)]
pub fn estimate_message_tokens(message: JsValue) -> Result<f64, JsValue> {
	let message: Message = throw(from_js_text(&message))?;
	Ok(estimate::estimate_message_tokens(&message))
}

/// `estimateContextTokens(context)`: `context` is a `TranscriptContext` or a message list.
#[wasm_bindgen(js_name = aiEstimateContextTokens)]
pub fn estimate_context_tokens(context: JsValue) -> Result<JsValue, JsValue> {
	let messages = if Reflect::has(&context, &"messages".into()).unwrap_or(false) {
		Reflect::get(&context, &"messages".into())?
	} else {
		context
	};
	let messages: Vec<Message> = throw(from_js_text(&messages))?;
	throw(to_js(&estimate::estimate_context_tokens(&messages)))
}
