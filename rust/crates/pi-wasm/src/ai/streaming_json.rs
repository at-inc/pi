//! Exports of `utils/streaming-json`: the two classes as handles. A snapshot crosses as JSON text each time it changes,
//! so a changed snapshot costs its whole size; an unchanged one returns the same JS value, as in the TS. A Rust string
//! cannot hold half of a surrogate pair, so a piece that ends in a high surrogate keeps it back for the next piece.

use std::rc::Rc;

use js_sys::{Array, JsString, Object, Reflect};
use pi_ai::JsonObject;
use pi_ai::utils::streaming_json::{JsonChange, StreamingJsonParser, ToolCallArgumentsStream, drop_deep};
use serde_json::Value;
use wasm_bindgen::prelude::*;

use super::json_parse::parsed_arguments_to_js;
use crate::json::{to_js_text, value_to_js};
use crate::options::throw;

/// `StreamingJsonParser`.
#[wasm_bindgen(js_name = AiStreamingJsonParser)]
pub struct StreamingJsonParserHandle {
	/// The last snapshot and its JS value. Declared before `parser`, so it is dropped first and the parser's drop
	/// releases deep values without recursion.
	snapshot: Option<(Rc<Value>, JsValue)>,
	parser: StreamingJsonParser,
	pieces: Pieces,
}

#[wasm_bindgen(js_class = AiStreamingJsonParser)]
impl StreamingJsonParserHandle {
	/// `new StreamingJsonParser()`.
	#[wasm_bindgen(constructor)]
	pub fn new() -> Self {
		Self {
			snapshot: None,
			parser: StreamingJsonParser::new(),
			pieces: Pieces::default(),
		}
	}

	/// `append(text)`: the changes, `{ kind: "set", path, value }` or `{ kind: "append", path, text }`.
	pub fn append(&mut self, text: &JsString) -> Result<Array, JsValue> {
		let changes = Array::new();
		for change in self.parser.append(&self.pieces.next(text)) {
			let js = Object::new();
			match change {
				JsonChange::Set { path, value } => {
					Reflect::set(&js, &"kind".into(), &"set".into())?;
					Reflect::set(&js, &"path".into(), &throw(to_js_text(&path))?)?;
					Reflect::set(&js, &"value".into(), &throw(value_to_js(&value))?)?;
					drop_deep(value);
				}
				JsonChange::Append { path, text } => {
					Reflect::set(&js, &"kind".into(), &"append".into())?;
					Reflect::set(&js, &"path".into(), &throw(to_js_text(&path))?)?;
					Reflect::set(&js, &"text".into(), &text.into())?;
				}
			}
			changes.push(&js);
		}
		Ok(changes)
	}

	/// `snapshot()`: `undefined` before a value starts.
	pub fn snapshot(&mut self) -> Result<JsValue, JsValue> {
		let Some(snapshot) = self.parser.snapshot() else {
			return Ok(JsValue::UNDEFINED);
		};
		if let Some((last, js)) = &self.snapshot
			&& Rc::ptr_eq(last, &snapshot)
		{
			return Ok(js.clone());
		}
		let js = throw(value_to_js(&snapshot))?;
		if let Some((last, _)) = self.snapshot.replace((snapshot, js.clone()))
			&& let Ok(last) = Rc::try_unwrap(last)
		{
			drop_deep(last);
		}
		Ok(js)
	}
}

impl Default for StreamingJsonParserHandle {
	fn default() -> Self {
		Self::new()
	}
}

/// `ToolCallArgumentsStream`. The Rust stream updates one arguments object in place; the handle hands out a new JS
/// object whenever it was written, as the TS returns a new snapshot when the arguments change.
#[wasm_bindgen(js_name = AiToolCallArgumentsStream)]
pub struct ToolCallArgumentsStreamHandle {
	stream: ToolCallArgumentsStream,
	arguments: JsonObject,
	js: JsValue,
	pieces: Pieces,
}

#[wasm_bindgen(js_class = AiToolCallArgumentsStream)]
impl ToolCallArgumentsStreamHandle {
	/// `new ToolCallArgumentsStream()`.
	#[wasm_bindgen(constructor)]
	pub fn new() -> Self {
		Self {
			stream: ToolCallArgumentsStream::new(),
			arguments: JsonObject::new(),
			js: Object::new().into(),
			pieces: Pieces::default(),
		}
	}

	/// `text`.
	#[wasm_bindgen(getter)]
	pub fn text(&self) -> String {
		self.stream.text().to_string()
	}

	/// `append(delta)`.
	pub fn append(&mut self, delta: &JsString) -> Result<JsValue, JsValue> {
		let delta = self.pieces.next(delta);
		self.append_text(&delta)
	}

	/// `replace(text)`: `{ delta, arguments }`. `delta` is `undefined` when `text` does not continue the text so far.
	pub fn replace(&mut self, text: &JsString) -> Result<JsValue, JsValue> {
		let text = self.pieces.whole(text);
		let delta = text.strip_prefix(self.stream.text()).map(str::to_string);
		let arguments = match &delta {
			Some(delta) => self.append_text(delta)?,
			None => {
				self.stream = ToolCallArgumentsStream::new();
				drop_deep(Value::Object(std::mem::take(&mut self.arguments)));
				self.stream.append(&text, &mut self.arguments);
				self.write_js()?
			}
		};
		let result = Object::new();
		Reflect::set(
			&result,
			&"delta".into(),
			&delta.map_or(JsValue::UNDEFINED, JsValue::from),
		)?;
		Reflect::set(&result, &"arguments".into(), &arguments)?;
		Ok(result.into())
	}

	/// `finish()`.
	pub fn finish(&self) -> Result<JsValue, JsValue> {
		parsed_arguments_to_js(self.stream.finish())
	}
}

impl ToolCallArgumentsStreamHandle {
	/// Appends `delta` and returns the JS arguments: the previous object unless the arguments were written.
	fn append_text(&mut self, delta: &str) -> Result<JsValue, JsValue> {
		if delta.is_empty() || !self.stream.append_written(delta, &mut self.arguments) {
			return Ok(self.js.clone());
		}
		self.write_js()
	}

	/// A new JS object of the current arguments.
	fn write_js(&mut self) -> Result<JsValue, JsValue> {
		let arguments = Value::Object(std::mem::take(&mut self.arguments));
		let js = value_to_js(&arguments);
		if let Value::Object(arguments) = arguments {
			self.arguments = arguments;
		}
		self.js = throw(js)?;
		Ok(self.js.clone())
	}
}

impl Default for ToolCallArgumentsStreamHandle {
	fn default() -> Self {
		Self::new()
	}
}

impl Drop for ToolCallArgumentsStreamHandle {
	fn drop(&mut self) {
		drop_deep(Value::Object(std::mem::take(&mut self.arguments)));
	}
}

/// Pieces of a JS string stream as Rust strings: a high surrogate that ends a piece is held back and joined with the
/// low surrogate that starts the next one. Other unpaired surrogates read as U+FFFD.
#[derive(Default)]
struct Pieces {
	high_surrogate: Option<u16>,
}

impl Pieces {
	/// The next piece, with the held back high surrogate in front and a trailing high surrogate held back.
	fn next(&mut self, piece: &JsString) -> String {
		let mut units: Vec<u16> = self.high_surrogate.take().into_iter().chain(piece.iter()).collect();
		if units.last().is_some_and(|unit| (0xd800..=0xdbff).contains(unit)) {
			self.high_surrogate = units.pop();
		}
		String::from_utf16_lossy(&units)
	}

	/// A whole text, which replaces what came before: nothing is held back.
	fn whole(&mut self, text: &JsString) -> String {
		self.high_surrogate = None;
		String::from(text)
	}
}
