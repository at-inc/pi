//! Exports of `utils/text`.

use pi_ai::SystemMessage;
use pi_ai::utils::text::{self, TextBlock};
use serde::Deserialize;
use wasm_bindgen::prelude::*;

use crate::json::from_js_text;
use crate::options::{arg, throw};

/// `contentText(content, separator = "\n")`: `content` is a string or any content block array. Only each block's
/// `type` and `text` are read, so image data does not cross.
#[wasm_bindgen(js_name = aiContentText)]
pub fn content_text(content: JsValue, separator: Option<String>) -> Result<JsValue, JsValue> {
	if content.is_string() {
		return Ok(content);
	}
	let blocks: Vec<Block> = arg(content)?;
	Ok(text::content_text(blocks.as_slice(), separator.as_deref().unwrap_or("\n")).into())
}

/// A content block as `contentText` reads it.
#[derive(Deserialize)]
struct Block {
	#[serde(rename = "type")]
	kind: Option<String>,
	text: Option<String>,
}

impl TextBlock for Block {
	fn block_text(&self) -> Option<&str> {
		(self.kind.as_deref() == Some("text")).then(|| self.text.as_deref().unwrap_or_default())
	}
}

/// `getSystemMessageText(message)`.
#[wasm_bindgen(js_name = aiGetSystemMessageText)]
pub fn get_system_message_text(message: JsValue) -> Result<String, JsValue> {
	let message: SystemMessage = throw(from_js_text(&message))?;
	Ok(text::get_system_message_text(&message))
}

/// `renderSystemMessageUpdate(message)`.
#[wasm_bindgen(js_name = aiRenderSystemMessageUpdate)]
pub fn render_system_message_update(message: JsValue) -> Result<String, JsValue> {
	let message: SystemMessage = throw(from_js_text(&message))?;
	Ok(text::render_system_message_update(&message))
}

/// `ownedSlice(text, start, end = text.length)`, with `String.prototype.slice`'s index rules.
#[wasm_bindgen(js_name = aiOwnedSlice)]
pub fn owned_slice(text: &str, start: f64, end: Option<f64>) -> String {
	let length = pi_host::js::utf16_len(text);
	let start = slice_index(start, length);
	let end = end.map_or(length, |end| slice_index(end, length));
	if end <= start {
		return String::new();
	}
	text::owned_slice(text, start, Some(end))
}

/// A `slice` argument as an index into `length` units: truncated, negative counts from the end, clamped.
fn slice_index(index: f64, length: usize) -> usize {
	let index = if index.is_nan() { 0.0 } else { index.trunc() };
	let resolved = if index < 0.0 { length as f64 + index } else { index };
	resolved.clamp(0.0, length as f64) as usize
}
