//! JavaScript-compatible helpers: `JSON.stringify`, `String(number)`, and UTF-16 string lengths.
//!
//! Ported code must use these wherever TypeScript output is observable (request bodies, tool-call argument
//! strings, error messages, token estimates), so the Rust port produces the same bytes as the TS original.

use std::io;

use serde::Serialize;

/// `JSON.stringify(value)`: compact, JavaScript key order (see `js_key_order`), and numbers formatted like
/// JavaScript (`1`, not `1.0`; `1e+21`, not `1e21`).
pub fn json_stringify<T: Serialize + ?Sized>(value: &T) -> String {
	let value = js_value(value);
	let mut out = Vec::new();
	let mut serializer = serde_json::Serializer::with_formatter(&mut out, JsFormatter::compact());
	value
		.serialize(&mut serializer)
		.expect("JSON.stringify: serialization of in-memory value failed");
	String::from_utf8(out).expect("serde_json produced invalid UTF-8")
}

/// `JSON.stringify(value, null, indent)` with an indent of `indent` spaces.
pub fn json_stringify_pretty<T: Serialize + ?Sized>(value: &T, indent: usize) -> String {
	let value = js_value(value);
	let mut out = Vec::new();
	let indent = " ".repeat(indent);
	let mut serializer = serde_json::Serializer::with_formatter(&mut out, JsFormatter::pretty(indent.as_bytes()));
	value
		.serialize(&mut serializer)
		.expect("JSON.stringify: serialization of in-memory value failed");
	String::from_utf8(out).expect("serde_json produced invalid UTF-8")
}

fn js_value<T: Serialize + ?Sized>(value: &T) -> serde_json::Value {
	let mut value = serde_json::to_value(value).expect("JSON.stringify: serialization of in-memory value failed");
	js_key_order(&mut value);
	value
}

/// Reorders every object in `value` the way JavaScript orders own property keys: array-index keys (`"0"`,
/// `"42"`, up to 2^32 - 2, no leading zeros) first in ascending numeric order, then the other keys in insertion
/// order.
pub fn js_key_order(value: &mut serde_json::Value) {
	match value {
		serde_json::Value::Array(items) => items.iter_mut().for_each(js_key_order),
		serde_json::Value::Object(map) => {
			map.values_mut().for_each(js_key_order);
			if !map.keys().any(|key| array_index(key).is_some()) {
				return;
			}
			let entries: Vec<_> = std::mem::take(map).into_iter().collect();
			let (mut indices, others): (Vec<_>, Vec<_>) =
				entries.into_iter().partition(|(key, _)| array_index(key).is_some());
			indices.sort_by_key(|(key, _)| array_index(key));
			map.extend(indices);
			map.extend(others);
		}
		_ => {}
	}
}

fn array_index(key: &str) -> Option<u32> {
	if key.is_empty() || (key.len() > 1 && key.starts_with('0')) || !key.bytes().all(|byte| byte.is_ascii_digit()) {
		return None;
	}
	key.parse::<u32>().ok().filter(|index| *index != u32::MAX)
}

/// `String(n)` / template-literal interpolation of a number.
pub fn number_to_string(value: f64) -> String {
	if value.is_nan() {
		return "NaN".into();
	}
	if value.is_infinite() {
		return if value > 0.0 {
			"Infinity".into()
		} else {
			"-Infinity".into()
		};
	}
	let mut buffer = ryu_js::Buffer::new();
	buffer.format(value).to_string()
}

/// `str.length`: the number of UTF-16 code units.
pub fn utf16_len(value: &str) -> usize {
	value.encode_utf16().count()
}

/// `str.slice(0, n)` in UTF-16 code units. A cut inside a surrogate pair drops the lone half, since Rust
/// strings cannot hold it (JavaScript would keep a lone surrogate).
pub fn utf16_prefix(value: &str, units: usize) -> &str {
	let mut seen = 0;
	for (index, ch) in value.char_indices() {
		let width = ch.len_utf16();
		if seen + width > units {
			return &value[..index];
		}
		seen += width;
	}
	value
}

/// Deserializes `x?: T | null` into `Option<Option<T>>`: absent -> `None`, `null` -> `Some(None)`. Use with
/// `#[serde(default, deserialize_with = "pi_host::js::double_option", skip_serializing_if = "Option::is_none")]`.
pub fn double_option<'de, D, T>(deserializer: D) -> Result<Option<Option<T>>, D::Error>
where
	D: serde::Deserializer<'de>,
	T: serde::Deserialize<'de>,
{
	<Option<T> as serde::Deserialize>::deserialize(deserializer).map(Some)
}

/// JSON number formatting that matches JavaScript.
struct JsFormatter<'a> {
	pretty: Option<serde_json::ser::PrettyFormatter<'a>>,
}

impl<'a> JsFormatter<'a> {
	fn compact() -> Self {
		Self { pretty: None }
	}

	fn pretty(indent: &'a [u8]) -> Self {
		Self {
			pretty: Some(serde_json::ser::PrettyFormatter::with_indent(indent)),
		}
	}
}

macro_rules! delegate {
	($($name:ident($($arg:ident: $ty:ty),*);)*) => {
		$(fn $name<W: ?Sized + io::Write>(&mut self, writer: &mut W $(, $arg: $ty)*) -> io::Result<()> {
			match &mut self.pretty {
				Some(pretty) => serde_json::ser::Formatter::$name(pretty, writer $(, $arg)*),
				None => serde_json::ser::Formatter::$name(&mut serde_json::ser::CompactFormatter, writer $(, $arg)*),
			}
		})*
	};
}

impl serde_json::ser::Formatter for JsFormatter<'_> {
	fn write_f64<W: ?Sized + io::Write>(&mut self, writer: &mut W, value: f64) -> io::Result<()> {
		if !value.is_finite() {
			return writer.write_all(b"null");
		}
		writer.write_all(number_to_string(value).as_bytes())
	}

	fn write_f32<W: ?Sized + io::Write>(&mut self, writer: &mut W, value: f32) -> io::Result<()> {
		self.write_f64(writer, f64::from(value))
	}

	delegate! {
		begin_array();
		end_array();
		begin_array_value(first: bool);
		end_array_value();
		begin_object();
		end_object();
		begin_object_key(first: bool);
		end_object_key();
		begin_object_value();
		end_object_value();
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn stringify_matches_javascript_numbers_and_order() {
		let value = serde_json::json!({ "b": 1.0, "a": [0.5, 1e21, 3], "c": "é" });
		assert_eq!(json_stringify(&value), r#"{"b":1,"a":[0.5,1e+21,3],"c":"é"}"#);
		assert_eq!(
			json_stringify_pretty(&serde_json::json!({ "a": 1.0 }), 2),
			"{\n  \"a\": 1\n}"
		);
	}

	#[test]
	fn utf16_helpers() {
		assert_eq!(utf16_len("a😀"), 3);
		assert_eq!(utf16_prefix("a😀b", 2), "a");
		assert_eq!(utf16_prefix("a😀b", 3), "a😀");
		assert_eq!(number_to_string(0.1 + 0.2), "0.30000000000000004");
	}
}
