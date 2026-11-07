//! Exports of `utils/error-body`. A thrown `Error` is read as the `PiError` the Rust function takes, after the
//! checks only JS can make: an `error` or `$response.body` field that is not a plain object (a class instance, a
//! stream) is no body, as in the TS.

use js_sys::{JSON, Object, Reflect};
use pi_ai::utils::error_body::{self, NormalizedProviderError};
use serde_json::Value;
use wasm_bindgen::prelude::*;

#[wasm_bindgen]
extern "C" {
	/// `String(value)`.
	#[wasm_bindgen(js_name = String)]
	fn js_string(value: &JsValue) -> String;
}

/// `MAX_PROVIDER_ERROR_BODY_CHARS`.
#[wasm_bindgen(js_name = aiMaxProviderErrorBodyChars)]
pub fn max_provider_error_body_chars() -> f64 {
	error_body::MAX_PROVIDER_ERROR_BODY_CHARS as f64
}

/// `normalizeProviderError(error)`.
#[wasm_bindgen(js_name = aiNormalizeProviderError)]
pub fn normalize_provider_error(error: JsValue) -> Result<JsValue, JsValue> {
	if !error.is_instance_of::<js_sys::Error>() {
		return normalized_to_js(&NormalizedProviderError {
			status: None,
			body: None,
			message: safe_json_stringify(&error),
			message_carries_body: false,
		});
	}
	let mut pi_error = crate::error::from_js(error.clone());
	if let Some(Value::Object(data)) = &mut pi_error.data {
		if !is_plain_object(&Reflect::get(&error, &"error".into())?) {
			data.remove("error");
		}
		let response = Reflect::get(&error, &"$response".into())?;
		if response.is_object()
			&& !is_plain_object(&Reflect::get(&response, &"body".into())?)
			&& let Some(Value::Object(response)) = data.get_mut("$response")
			&& !response.get("body").is_some_and(Value::is_string)
		{
			response.remove("body");
		}
	}
	normalized_to_js(&error_body::normalize_provider_error(&pi_error))
}

/// `formatProviderError(norm, prefix)`.
#[wasm_bindgen(js_name = aiFormatProviderError)]
pub fn format_provider_error(norm: JsValue, prefix: Option<String>) -> Result<String, JsValue> {
	let field = |name: &str| Reflect::get(&norm, &name.into());
	let norm = NormalizedProviderError {
		status: field("status")?.as_f64(),
		body: field("body")?.as_string(),
		message: field("message")?.as_string().unwrap_or_default(),
		message_carries_body: field("messageCarriesBody")?.is_truthy(),
	};
	Ok(error_body::format_provider_error(&norm, prefix.as_deref()))
}

/// `truncateErrorText(text, maxChars)`.
#[wasm_bindgen(js_name = aiTruncateErrorText)]
pub fn truncate_error_text(text: &str, max_chars: f64) -> String {
	error_body::truncate_error_text(text, max_chars.max(0.0) as usize)
}

/// `safeJsonStringify(value)`: `JSON.stringify(value)`, or `String(value)` when that is `undefined` or throws.
#[wasm_bindgen(js_name = aiSafeJsonStringify)]
pub fn safe_json_stringify(value: &JsValue) -> String {
	match JSON::stringify(value) {
		Ok(text) => text.as_string().unwrap_or_else(|| js_string(value)),
		Err(_) => js_string(value),
	}
}

/// `isPlainNonEmptyObject`'s prototype check: an object whose prototype is `Object.prototype` or `null`.
fn is_plain_object(value: &JsValue) -> bool {
	if !value.is_object() {
		return false;
	}
	let prototype = Object::get_prototype_of(value);
	prototype.is_null() || prototype == Object::get_prototype_of(&Object::new())
}

/// `{ status, body, message, messageCarriesBody }`.
fn normalized_to_js(norm: &NormalizedProviderError) -> Result<JsValue, JsValue> {
	let result = Object::new();
	Reflect::set(
		&result,
		&"status".into(),
		&norm.status.map_or(JsValue::UNDEFINED, JsValue::from),
	)?;
	Reflect::set(
		&result,
		&"body".into(),
		&norm.body.as_deref().map_or(JsValue::UNDEFINED, JsValue::from),
	)?;
	Reflect::set(&result, &"message".into(), &norm.message.as_str().into())?;
	Reflect::set(&result, &"messageCarriesBody".into(), &norm.message_carries_body.into())?;
	Ok(result.into())
}
