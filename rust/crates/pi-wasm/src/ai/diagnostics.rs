//! Exports of `utils/diagnostics`, and `thrown`: a thrown JS value as the `Thrown` the Rust functions take.

use js_sys::{Array, Object, Reflect};
use pi_ai::JsonObject;
use pi_ai::utils::diagnostics::{self, Thrown};
use pi_host::PiError;
use serde_json::Value;
use wasm_bindgen::JsCast;
use wasm_bindgen::prelude::*;

use crate::json::{from_js_text, to_js};
use crate::options::throw;

#[wasm_bindgen]
extern "C" {
	/// The global `Headers` class (`instanceof` is `false` where it does not exist).
	#[wasm_bindgen(js_name = Headers)]
	type JsHeaders;
	/// `String(value)`.
	#[wasm_bindgen(js_name = String)]
	fn js_string(value: &JsValue) -> String;
}

/// A thrown JS value as Rust reads it: an `Error` (`instanceof Error`) becomes a `PiError` whose `data` holds its own
/// fields, a `Headers` field as `Object.fromEntries(headers)` and a numeric `status` as it is; a plain object or array
/// is its JSON value; any other value is `String(value)`.
pub(super) enum ThrownJs {
	Error(PiError),
	Value(Value),
}

impl ThrownJs {
	pub(super) fn new(value: &JsValue) -> Self {
		if !value.is_instance_of::<js_sys::Error>() {
			if value.is_object() {
				return Self::Value(from_js_text(value).unwrap_or_else(|_| Value::String(js_string(value))));
			}
			return Self::Value(Value::String(js_string(value)));
		}
		let mut error = crate::error::from_js(value.clone());
		let mut data = match error.data.take() {
			Some(Value::Object(data)) => data,
			_ => serde_json::Map::new(),
		};
		if let Ok(headers) = Reflect::get(value, &"headers".into())
			&& headers.is_instance_of::<JsHeaders>()
			&& let Ok(entries) = Object::from_entries(&headers)
			&& let Ok(headers) = from_js_text(&entries)
		{
			data.insert("headers".into(), headers);
		}
		if let Some(status) = Reflect::get(value, &"status".into())
			.ok()
			.and_then(|status| status.as_f64())
		{
			error.status = None;
			data.insert("status".into(), Value::from(status));
		}
		if !data.is_empty() {
			error.data = Some(Value::Object(data));
		}
		Self::Error(error)
	}

	pub(super) fn as_thrown(&self) -> Thrown<'_> {
		match self {
			Self::Error(error) => Thrown::Error(error),
			Self::Value(value) => Thrown::Value(value),
		}
	}
}

/// `formatThrownValue(value)`.
#[wasm_bindgen(js_name = aiFormatThrownValue)]
pub fn format_thrown_value(value: JsValue) -> String {
	diagnostics::format_thrown_value(ThrownJs::new(&value).as_thrown())
}

/// `extractDiagnosticError(error)`.
#[wasm_bindgen(js_name = aiExtractDiagnosticError)]
pub fn extract_diagnostic_error(error: JsValue) -> Result<JsValue, JsValue> {
	let info = throw(to_js(&diagnostics::extract_diagnostic_error(
		ThrownJs::new(&error).as_thrown(),
	)))?;
	with_stack(&info, &error)?;
	Ok(info)
}

/// `createAssistantMessageDiagnostic(type, error, details)`.
#[wasm_bindgen(js_name = aiCreateAssistantMessageDiagnostic)]
pub fn create_assistant_message_diagnostic(kind: &str, error: JsValue, details: JsValue) -> Result<JsValue, JsValue> {
	let details: Option<JsonObject> = throw(from_js_text(&details))?;
	let diagnostic = diagnostics::create_assistant_message_diagnostic(kind, ThrownJs::new(&error).as_thrown(), details);
	diagnostic_to_js(&diagnostic, &error)
}

/// `createProviderErrorDiagnostic(error)`.
#[wasm_bindgen(js_name = aiCreateProviderErrorDiagnostic)]
pub fn create_provider_error_diagnostic(error: JsValue) -> Result<JsValue, JsValue> {
	let diagnostic = diagnostics::create_provider_error_diagnostic(ThrownJs::new(&error).as_thrown());
	diagnostic_to_js(&diagnostic, &error)
}

/// `appendAssistantMessageDiagnostic(message, diagnostic)`: sets `message.diagnostics` to a new array that ends with
/// `diagnostic`, the caller's object.
#[wasm_bindgen(js_name = aiAppendAssistantMessageDiagnostic)]
pub fn append_assistant_message_diagnostic(message: JsValue, diagnostic: JsValue) -> Result<(), JsValue> {
	let existing = Reflect::get(&message, &"diagnostics".into())?;
	let diagnostics = if existing.is_undefined() || existing.is_null() {
		Array::new()
	} else {
		Array::from(&existing)
	};
	diagnostics.push(&diagnostic);
	Reflect::set(&message, &"diagnostics".into(), &diagnostics)?;
	Ok(())
}

/// The JS diagnostic, with the error's `stack`, which Rust errors do not carry.
fn diagnostic_to_js(diagnostic: &diagnostics::AssistantMessageDiagnostic, error: &JsValue) -> Result<JsValue, JsValue> {
	let js = throw(to_js(diagnostic))?;
	with_stack(&Reflect::get(&js, &"error".into())?, error)?;
	Ok(js)
}

/// Sets `info.stack` to the stack of `error` when it is an `Error` with one, as `extractDiagnosticError` does.
fn with_stack(info: &JsValue, error: &JsValue) -> Result<(), JsValue> {
	if !error.is_instance_of::<js_sys::Error>() || !info.is_object() {
		return Ok(());
	}
	let stack = Reflect::get(error, &"stack".into())?;
	if !stack.is_undefined() {
		Reflect::set(info.unchecked_ref(), &"stack".into(), &stack)?;
	}
	Ok(())
}
