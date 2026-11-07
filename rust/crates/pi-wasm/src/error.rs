//! Errors across the boundary: a thrown JS value becomes a `PiError` with the JS `name` and `message`, and a
//! `PiError` becomes a JS `Error` with the same `name`, `message`, `status` and data fields.

use js_sys::{Error, Object, Reflect};
use pi_host::PiError;
use serde_json::{Map, Value};
use wasm_bindgen::JsCast;
use wasm_bindgen::prelude::*;

#[wasm_bindgen]
extern "C" {
	/// `String(value)`.
	#[wasm_bindgen(js_name = String)]
	fn js_string(value: &JsValue) -> String;
}

/// Properties of a JS error that are not data.
const ERROR_FIELDS: [&str; 4] = ["name", "message", "stack", "status"];

/// The `PiError` for a thrown JS value. An `Error` (or an object with a string `message`, like a `DOMException` in
/// runtimes where it is not an `Error`) keeps its `name`, `message` and numeric `status`; its other own enumerable
/// properties that are plain data (`code`, `cause`, ...) go to `data`. Any other value is `new Error(String(value))`.
pub fn from_js(value: JsValue) -> PiError {
	let message = Reflect::get(&value, &"message".into())
		.ok()
		.and_then(|message| message.as_string());
	let Some(message) = message.filter(|_| value.is_object()) else {
		return PiError::new(js_string(&value));
	};
	let name = Reflect::get(&value, &"name".into())
		.ok()
		.and_then(|name| name.as_string())
		.unwrap_or_else(|| "Error".into());
	let mut error = PiError::named(name, message);
	if let Some(status) = Reflect::get(&value, &"status".into())
		.ok()
		.and_then(|status| status.as_f64())
	{
		error.status = Some(status as u16);
	}
	let mut data = Map::new();
	for key in Object::keys(value.unchecked_ref::<Object>()) {
		let Some(key) = key.as_string() else { continue };
		if ERROR_FIELDS.contains(&key.as_str()) {
			continue;
		}
		let Ok(field) = Reflect::get(&value, &key.as_str().into()) else {
			continue;
		};
		if field.is_undefined() || field.is_function() {
			continue;
		}
		if let Ok(field) = crate::json::from_js::<Value>(field) {
			data.insert(key, field);
		}
	}
	if !data.is_empty() {
		error.data = Some(Value::Object(data));
	}
	error
}

/// The JS `Error` for a `PiError` (a `TypeError`, `RangeError` or `SyntaxError` for those names): `name` and `message` as given, `status` when set, and the fields of an object
/// `data` as own properties, the inverse of `from_js`.
pub fn to_js(error: &PiError) -> JsValue {
	let js: Error = match error.name.as_str() {
		"TypeError" => js_sys::TypeError::new(&error.message).into(),
		"RangeError" => js_sys::RangeError::new(&error.message).into(),
		"SyntaxError" => js_sys::SyntaxError::new(&error.message).into(),
		_ => Error::new(&error.message),
	};
	if js.name() != error.name.as_str() {
		js.set_name(&error.name);
	}
	if let Some(status) = error.status {
		let _ = Reflect::set(&js, &"status".into(), &JsValue::from(status));
	}
	if let Some(Value::Object(data)) = &error.data {
		for (key, value) in data {
			if let Ok(value) = crate::json::to_js(value) {
				let _ = Reflect::set(&js, &key.as_str().into(), &value);
			}
		}
	}
	js.into()
}
