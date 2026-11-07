//! Exports of `api/transform-messages`.

use std::cell::RefCell;
use std::rc::Rc;

use js_sys::Function;
use pi_ai::api::transform_messages;
use pi_ai::{AssistantMessage, Message, Model, ToolResultMessage};
use serde_json::json;
use wasm_bindgen::JsCast;
use wasm_bindgen::prelude::*;

use crate::options::{arg, throw};

/// `splitErrorToolResultImages(msg)`: `{ content, caption, images }`, or `undefined` for a result that needs no split.
#[wasm_bindgen(js_name = aiSplitErrorToolResultImages)]
pub fn split_error_tool_result_images(msg: JsValue) -> Result<JsValue, JsValue> {
	let msg: ToolResultMessage = arg(msg)?;
	let Some(split) = transform_messages::split_error_tool_result_images(&msg) else {
		return Ok(JsValue::UNDEFINED);
	};
	throw(crate::json::to_js(&json!({
		"content": split.content,
		"caption": split.caption,
		"images": split.images,
	})))
}

/// `transformMessages(messages, model, normalizeToolCallId)`. The callback is called synchronously with the caller's
/// model object and the source message as plain data; its first throw is rethrown once the transform returns.
#[wasm_bindgen(js_name = aiTransformMessages)]
pub fn transform_messages(
	messages: JsValue,
	model: JsValue,
	normalize_tool_call_id: JsValue,
) -> Result<JsValue, JsValue> {
	let messages: Vec<Message> = arg(messages)?;
	let parsed: Model = super::model(&model)?;
	let thrown: Rc<RefCell<Option<JsValue>>> = Rc::default();
	let normalize = normalize_tool_call_id.dyn_ref::<Function>().map(|function| {
		let (function, model, thrown) = (function.clone(), model.clone(), thrown.clone());
		move |id: &str, _: &Model, source: &AssistantMessage| -> String {
			if thrown.borrow().is_some() {
				return id.to_string();
			}
			let result = crate::json::to_js(source)
				.map_err(|error| crate::error::to_js(&error))
				.and_then(|source| function.call3(&JsValue::UNDEFINED, &id.into(), &model, &source));
			match result {
				Ok(normalized) => normalized.as_string().unwrap_or_else(|| id.to_string()),
				Err(error) => {
					*thrown.borrow_mut() = Some(error);
					id.to_string()
				}
			}
		}
	});
	let transformed = transform_messages::transform_messages(
		&messages,
		&parsed,
		normalize
			.as_ref()
			.map(|normalize| normalize as &transform_messages::NormalizeToolCallId),
	);
	if let Some(error) = thrown.take() {
		return Err(error);
	}
	throw(crate::json::to_js(&transformed))
}
