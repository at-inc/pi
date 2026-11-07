//! Plain data across the boundary: serde values to JS values and back with `serde-wasm-bindgen`, using the Rust
//! serde shapes, which match the TS JSON. Maps become plain objects and `None`/`null` become `null`. The `*_text`
//! variants cross as JSON text instead, with `JSON.stringify` and `JSON.parse` semantics.

use pi_host::{PiError, PiResult};
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::Value;
use wasm_bindgen::JsValue;

const SERIALIZER: serde_wasm_bindgen::Serializer = serde_wasm_bindgen::Serializer::new()
	.serialize_maps_as_objects(true)
	.serialize_missing_as_null(true);

/// The JS value of a serde value.
pub fn to_js<T: Serialize + ?Sized>(value: &T) -> PiResult<JsValue> {
	value
		.serialize(&SERIALIZER)
		.map_err(|error| PiError::type_error(error.to_string()))
}

/// A serde value from a JS value. JS `undefined` and `null` both read as `null`/`None`; functions are rejected, so
/// callers strip them from objects that carry callbacks (see `options`).
pub fn from_js<T: DeserializeOwned>(value: JsValue) -> PiResult<T> {
	serde_wasm_bindgen::from_value(value).map_err(|error| PiError::type_error(error.to_string()))
}

/// A serde value from a JS value read as `JSON.stringify` writes it: functions, `undefined` and symbols are dropped from
/// objects, `toJSON` applies, and non-enumerable properties (TypeBox's kind marks) are skipped, as when the TS JSON-copies
/// a value. A lone surrogate in a string reads as U+FFFD. Faster than `from_js` for large values: `JSON.stringify` is
/// native and `serde_json` parses the text in one pass.
pub fn from_js_text<T: DeserializeOwned>(value: &JsValue) -> PiResult<T> {
	let text = js_sys::JSON::stringify(value).map_err(crate::error::from_js)?;
	// `JSON.stringify(undefined)` (and of a function) is `undefined`, which reads as `null`.
	let text = text.as_string().unwrap_or_else(|| "null".into());
	match serde_json::from_str(&text) {
		Ok(value) => Ok(value),
		Err(_) if text.contains("\\u") => serde_json::from_str(&without_lone_surrogate_escapes(&text))
			.map_err(|error| PiError::type_error(error.to_string())),
		Err(error) => Err(PiError::type_error(error.to_string())),
	}
}

/// JSON text with every `\u` escape of an unpaired surrogate replaced by `\ufffd`, which `serde_json` accepts.
fn without_lone_surrogate_escapes(text: &str) -> String {
	let bytes = text.as_bytes();
	let escape_at = |index: usize| -> Option<u16> {
		let digits = text.get(index + 2..index + 6)?;
		(bytes.get(index) == Some(&b'\\') && bytes.get(index + 1) == Some(&b'u'))
			.then(|| u16::from_str_radix(digits, 16).ok())
			.flatten()
	};
	let mut out = String::with_capacity(text.len());
	let mut index = 0;
	while index < bytes.len() {
		if bytes[index] != b'\\' {
			let next = text[index..].find('\\').map_or(text.len(), |offset| index + offset);
			out.push_str(&text[index..next]);
			index = next;
			continue;
		}
		match escape_at(index) {
			Some(0xd800..=0xdbff) if matches!(escape_at(index + 6), Some(0xdc00..=0xdfff)) => {
				out.push_str(&text[index..index + 12]);
				index += 12;
			}
			Some(0xd800..=0xdfff) => {
				out.push_str("\\ufffd");
				index += 6;
			}
			_ => {
				out.push_str(&text[index..(index + 2).min(text.len())]);
				index += 2;
			}
		}
	}
	out
}

/// The JS value of a serde value, built by `JSON.parse` of its JSON text: plain objects and arrays with own
/// `__proto__` members, as `JSON.parse` builds them. Non-finite numbers become `null`.
pub fn to_js_text<T: Serialize + ?Sized>(value: &T) -> PiResult<JsValue> {
	let text = serde_json::to_string(value).map_err(|error| PiError::type_error(error.to_string()))?;
	js_sys::JSON::parse(&text).map_err(crate::error::from_js)
}

/// `to_js_text` of a JSON value of any depth: the text is written without recursion, so a value nested deeper than the
/// wasm stack allows still converts. Numbers keep their sign (`-0`).
pub fn value_to_js(value: &Value) -> PiResult<JsValue> {
	js_sys::JSON::parse(&value_text(value)).map_err(crate::error::from_js)
}

/// The JSON text of `value`, written with an explicit stack.
fn value_text(value: &Value) -> String {
	enum Step<'a> {
		Value(&'a Value),
		Key(&'a str),
		Text(&'static str),
	}
	let mut out = String::new();
	let mut stack = vec![Step::Value(value)];
	while let Some(step) = stack.pop() {
		match step {
			Step::Text(text) => out.push_str(text),
			Step::Key(key) => {
				push_json_string(&mut out, key);
				out.push(':');
			}
			Step::Value(Value::Null) => out.push_str("null"),
			Step::Value(Value::Bool(value)) => out.push_str(if *value { "true" } else { "false" }),
			Step::Value(Value::Number(number)) => match number.as_f64() {
				Some(number) if number == 0.0 && number.is_sign_negative() => out.push_str("-0"),
				Some(number) if number.is_finite() => out.push_str(&pi_host::js::number_to_string(number)),
				_ => out.push_str("null"),
			},
			Step::Value(Value::String(text)) => push_json_string(&mut out, text),
			Step::Value(Value::Array(items)) => {
				out.push('[');
				stack.push(Step::Text("]"));
				for (index, item) in items.iter().enumerate().rev() {
					stack.push(Step::Value(item));
					if index > 0 {
						stack.push(Step::Text(","));
					}
				}
			}
			Step::Value(Value::Object(members)) => {
				out.push('{');
				stack.push(Step::Text("}"));
				for (index, (key, item)) in members.iter().enumerate().rev() {
					stack.push(Step::Value(item));
					stack.push(Step::Key(key));
					if index > 0 {
						stack.push(Step::Text(","));
					}
				}
			}
		}
	}
	out
}

fn push_json_string(out: &mut String, text: &str) {
	out.push_str(&serde_json::to_string(text).expect("a string serializes"));
}
