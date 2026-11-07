//! Port of `packages/ai/src/utils/streaming-json.ts`.
//! PORT-STATUS: ported
//!
//! Incremental, tolerant parsing of JSON text that arrives in pieces, such as streamed tool-call arguments.
//!
//! `StreamingJsonParser::append()` reads each piece once, so a stream costs time and memory linear in its length, and
//! `snapshot()` returns the value of the text read so far. Snapshots are plain JSON values and never change once
//! returned. The rules for incomplete text follow `parse_streaming_json()`:
//!
//! - An unterminated string shows the characters read so far; an unterminated escape is held back until complete.
//! - An unterminated container shows the members and elements read so far.
//! - A member whose key or value has not started is omitted.
//! - An unterminated literal shows its value once it is a prefix of `true`, `false` or `null`; an unterminated number
//!   shows its value when the digits read so far form a number (an exponent without digits is ignored).
//!
//! Malformed text is read leniently, the way `repair_json()` and `parse_streaming_json()` treat it: raw control
//! characters in strings are kept, an invalid escape such as `\d` stays literal text, missing commas and colons are
//! tolerated, values in invalid positions and unknown words are dropped, and text after the root value is ignored. The
//! parser never fails. Callers that need the exact value of the finished text parse it once, e.g. with
//! `parse_tool_call_arguments()`.
//!
//! Differences from `parse_streaming_json()` on incomplete text: trailing whitespace inside an unterminated string is
//! kept (it was trimmed), a string that contains a raw control character or an invalid escape stays visible while it
//! streams (it was dropped until the document was complete), and `NaN` and `Infinity` words are dropped.
//!
//! `append()` also returns the changes the piece made, for a writer that keeps its own copy of the value and applies
//! them instead of re-assigning snapshots: `Set` when a key, an array item or a finished value appears (a new string
//! is set once with its first text, a container empty), `Append` when the piece extended an open string. Unterminated
//! numbers and literals are not reported until they end. A container nested more than 32 levels deep is reported as
//! one `Set` of its whole value when it closes, so paths stay short.
//!
//! Cost: each `append()` earns copy credit proportional to its length plus a constant; `snapshot()` spends it on a
//! fresh copy and returns the previous snapshot while the copy costs more than the credit. So a document that keeps a
//! very large container open shows a preview that lags a little behind the text, and no input makes the total work
//! grow faster than the text.
//!
//! Rust differences: TS snapshots share completed containers and strings, so a copy costs the member and element count
//! of the open path. A `serde_json::Value` cannot share, so a Rust copy costs the whole value read so far: the open
//! path with everything below it, strings counted by length. Previews of large documents therefore lag more than in
//! TS, while the total work stays linear. Snapshots are `Rc<Value>`: an unchanged snapshot is the same `Rc`. A
//! Rust string cannot hold a lone surrogate: a high surrogate escape is held back until its low half arrives, and an
//! unpaired one becomes U+FFFD. Values nested so deep that recursion would overflow the stack are copied and dropped
//! without recursion inside the parser; a caller that keeps such a snapshot must drop it the same way.

use std::rc::Rc;
use std::sync::LazyLock;

use pi_host::js::utf16_len;
use regex::Regex;
use serde::Serialize;
use serde_json::{Map, Value};

use super::json_parse::{ParsedToolCallArguments, number_value, parse_tool_call_arguments, set_js_member};
use crate::types::JsonObject;

/// Where a value sits in its parent; absent for a value in an invalid position, which is read but dropped.
#[derive(Clone, Debug, PartialEq)]
enum Slot {
	Root,
	Member(String),
	Element(usize),
}

/// One segment of a [`JsonPath`]: a member key or an array index.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(untagged)]
pub enum JsonPathSegment {
	Key(String),
	Index(usize),
}

/// A path from the root value: member keys and array indices.
pub type JsonPath = Vec<JsonPathSegment>;

/// One change a piece of text made to the value; see the module comment.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum JsonChange {
	Set { path: JsonPath, value: Value },
	Append { path: JsonPath, text: String },
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Phase {
	Key,
	Colon,
	Value,
	Comma,
}

/// An open object or array. It holds its completed members and elements; its open child is placed when it ends.
struct Frame {
	container: Value,
	slot: Option<Slot>,
	/// Where the container sits, when its changes are reported.
	path: Option<JsonPath>,
	/// Whether changes inside it are reported; when not, its whole value is reported as it closes.
	reports_inside: bool,
	/// What a copy of the container with everything in it costs.
	cost: usize,
	/// Objects: the key read last, where the member value goes.
	key: Option<String>,
	phase: Phase,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Target {
	Key,
	Value,
}

/// An unterminated string.
struct OpenString {
	target: Target,
	slot: Option<Slot>,
	text: String,
	/// Escape read so far: "" for none, otherwise a backslash and up to five more characters.
	escape: String,
	/// A decoded high surrogate escape waiting for its low half.
	high_surrogate: Option<u16>,
	/// Where a value string sits, when its changes are reported.
	path: Option<JsonPath>,
	/// The index in `changes` of the change this string extends, while it is the newest change of the piece.
	change: Option<usize>,
}

// Changes are reported at most this deep, so a path costs O(1) to build.
const MAX_REPORTED_DEPTH: usize = 32;
// Copy credit per append and per appended character. The floor keeps small documents fresh at every snapshot.
const CREDIT_PER_APPEND: usize = 256;
const CREDIT_PER_CHARACTER: usize = 8;
// String bytes per unit of copy cost: copying string bytes is much cheaper than copying a node.
const STRING_BYTES_PER_COST: usize = 16;
// An unterminated number longer than this is not shown until it ends, so `snapshot()` stays cheap.
const MAX_PENDING_SCALAR_LENGTH: usize = 64;
static NUMBER: LazyLock<Regex> =
	LazyLock::new(|| Regex::new(r"^-?(?:0|[1-9][0-9]*)(?:\.[0-9]+)?(?:[eE][+-]?[0-9]+)?$").expect("valid regex"));
static NUMBER_WITHOUT_EXPONENT_DIGITS: LazyLock<Regex> =
	LazyLock::new(|| Regex::new(r"^(-?(?:0|[1-9][0-9]*)(?:\.[0-9]+)?)[eE][+-]?$").expect("valid regex"));
const LITERALS: [(&str, Value); 3] = [
	("true", Value::Bool(true)),
	("false", Value::Bool(false)),
	("null", Value::Null),
];

/// Incremental, tolerant JSON parser; see the module comment for its rules and costs.
#[derive(Default)]
pub struct StreamingJsonParser {
	root: Option<Rc<Value>>,
	done: bool,
	stack: Vec<Frame>,
	string: Option<OpenString>,
	scalar: String,
	scalar_length: usize,
	scalar_slot: Option<Slot>,
	/// Sum of `cost` over the open path: what a fresh snapshot copies.
	path_cost: usize,
	credit: usize,
	dirty: bool,
	snapshot: Option<Rc<Value>>,
	/// Changes of the piece being read.
	changes: Vec<JsonChange>,
}

impl StreamingJsonParser {
	pub fn new() -> Self {
		Self::default()
	}

	/// Read the next piece of text; returns the changes it made, in order. Costs time linear in `text.len()`.
	pub fn append(&mut self, text: &str) -> Vec<JsonChange> {
		self.credit += CREDIT_PER_APPEND + CREDIT_PER_CHARACTER * utf16_len(text);
		self.changes = Vec::new();
		if let Some(open) = &mut self.string {
			open.change = None;
		}
		let mut index = 0;
		while index < text.len() && !self.done {
			if self.string.is_some() {
				index = self.read_string(text, index);
			} else if !self.scalar.is_empty() {
				index = self.read_scalar(text, index);
			} else {
				let ch = text[index..].chars().next().expect("index is a char boundary");
				index += ch.len_utf8();
				self.structural(ch);
			}
		}
		std::mem::take(&mut self.changes)
	}

	/// The value of the text read so far: `None` before a value starts, otherwise a plain JSON value that never
	/// changes afterwards. Returns the previous snapshot while the text read since did not change the value, or while
	/// a fresh copy is not yet paid for (see the module comment).
	pub fn snapshot(&mut self) -> Option<Rc<Value>> {
		if !self.dirty {
			return self.snapshot.clone();
		}
		let pending_cost = self.pending_cost();
		let cost = self.path_cost + pending_cost.unwrap_or(0);
		if cost > self.credit {
			return self.snapshot.clone();
		}
		self.credit -= cost;
		self.dirty = false;
		let pending = pending_cost.and_then(|_| self.pending());
		let snapshot = self.copy_open_path(pending);
		if let Some(previous) = std::mem::replace(&mut self.snapshot, snapshot)
			&& let Ok(previous) = Rc::try_unwrap(previous)
		{
			drop_deep(previous);
		}
		self.snapshot.clone()
	}

	/// Read string characters from `start` until the string ends or the text does; returns the next index.
	fn read_string(&mut self, text: &str, start: usize) -> usize {
		let bytes = text.as_bytes();
		let mut run_start = start;
		let mut index = start;
		while index < bytes.len() {
			let byte = bytes[index];
			if self.string.as_ref().is_some_and(|open| !open.escape.is_empty()) {
				if self.continue_escape(byte) {
					index += 1;
					run_start = index;
					continue;
				}
				// Not part of the escape: read it as a normal string character below.
				run_start = index;
			}
			if byte == b'"' {
				if index > run_start {
					self.append_string(&text[run_start..index]);
				}
				self.end_string();
				return index + 1;
			}
			if byte == b'\\' {
				if index > run_start {
					self.append_string(&text[run_start..index]);
				}
				self.open_string().escape = "\\".into();
				run_start = index + 1;
			}
			index += 1;
		}
		if text.len() > run_start {
			self.append_string(&text[run_start..]);
		}
		text.len()
	}

	/// Read number or literal characters from `start` up to the next delimiter; returns the delimiter's index.
	fn read_scalar(&mut self, text: &str, start: usize) -> usize {
		let end = text.as_bytes()[start..]
			.iter()
			.position(|byte| is_delimiter(*byte))
			.map_or(text.len(), |offset| start + offset);
		if end > start {
			self.scalar.push_str(&text[start..end]);
			self.scalar_length += utf16_len(&text[start..end]);
			if self.scalar_slot.is_some() {
				self.dirty = true;
			}
		}
		if end < text.len() {
			self.end_scalar();
		}
		end
	}

	/// Handle one character outside strings.
	fn structural(&mut self, ch: char) {
		match ch {
			' ' | '\t' | '\n' | '\r' => {}
			'"' => self.start_string(),
			'{' => self.open_container(Value::Object(Map::new())),
			'[' => self.open_container(Value::Array(Vec::new())),
			'}' | ']' => self.close_container(),
			':' => {
				if let Some(frame) = self.stack.last_mut()
					&& frame.container.is_object()
					&& frame.phase == Phase::Colon
				{
					frame.phase = Phase::Value;
				}
			}
			',' => {
				if let Some(frame) = self.stack.last_mut() {
					frame.phase = if frame.container.is_array() {
						Phase::Value
					} else {
						Phase::Key
					};
				}
			}
			_ => {
				self.scalar = ch.to_string();
				self.scalar_length = ch.len_utf16();
				self.scalar_slot = self.value_slot();
				if self.scalar_slot.is_some() {
					self.dirty = true;
				}
			}
		}
	}

	/// Where a value starting now goes, or `None` when this position takes no value.
	fn value_slot(&self) -> Option<Slot> {
		let Some(frame) = self.stack.last() else {
			return self.root.is_none().then_some(Slot::Root);
		};
		if let Value::Array(elements) = &frame.container {
			return Some(Slot::Element(elements.len()));
		}
		// A missing colon is tolerated: `{"a" 1}` reads as `{"a": 1}`.
		match (&frame.key, frame.phase) {
			(Some(key), Phase::Value | Phase::Colon) => Some(Slot::Member(key.clone())),
			_ => None,
		}
	}

	fn start_string(&mut self) {
		// A missing comma is tolerated: `{"a": 1 "b": 2}` reads the second member.
		let is_key = self
			.stack
			.last()
			.is_some_and(|frame| frame.container.is_object() && matches!(frame.phase, Phase::Key | Phase::Comma));
		if is_key {
			self.string = Some(OpenString {
				target: Target::Key,
				slot: None,
				text: String::new(),
				escape: String::new(),
				high_surrogate: None,
				path: None,
				change: None,
			});
			return;
		}
		let slot = self.value_slot();
		let path = self.reported_path(slot.as_ref());
		if slot.is_some() {
			self.dirty = true;
		}
		let change = path.clone().map(|path| {
			self.changes.push(JsonChange::Set {
				path,
				value: Value::String(String::new()),
			});
			self.changes.len() - 1
		});
		self.string = Some(OpenString {
			target: Target::Value,
			slot,
			text: String::new(),
			escape: String::new(),
			high_surrogate: None,
			path,
			change,
		});
	}

	/// The path of a value at `slot` in the innermost open container, when its changes are reported.
	fn reported_path(&self, slot: Option<&Slot>) -> Option<JsonPath> {
		let segment = match slot? {
			Slot::Root => return Some(Vec::new()),
			Slot::Member(key) => JsonPathSegment::Key(key.clone()),
			Slot::Element(index) => JsonPathSegment::Index(*index),
		};
		let frame = self
			.stack
			.last()
			.expect("a member or element slot has an open container");
		if !frame.reports_inside {
			return None;
		}
		let mut path = frame.path.clone().expect("a container that reports inside has a path");
		path.push(segment);
		Some(path)
	}

	fn open_string(&mut self) -> &mut OpenString {
		self.string.as_mut().expect("a string is open")
	}

	/// Continue an escape with `byte`; returns false when `byte` is not part of it and must be read normally.
	fn continue_escape(&mut self, byte: u8) -> bool {
		let open = self.open_string();
		if open.escape == "\\" {
			if let Some(decoded) = simple_escape(byte) {
				open.escape.clear();
				self.append_string(decoded);
				return true;
			}
			if byte == b'u' {
				open.escape.push('u');
				return true;
			}
			// Invalid escape: keep the backslash as text and read the character normally, as repair_json() does.
			open.escape.clear();
			self.append_string("\\");
			return false;
		}
		if byte.is_ascii_hexdigit() {
			open.escape.push(char::from(byte));
			if open.escape.len() < 6 {
				return true;
			}
			let unit = u16::from_str_radix(&open.escape[2..], 16).expect("four hex digits");
			open.escape.clear();
			self.append_unit(unit);
			return true;
		}
		// `\u` without four hex digits: keep it as text and read the character normally, as repair_json() does.
		let literal = std::mem::take(&mut open.escape);
		self.append_string(&literal);
		false
	}

	/// Append a code unit decoded from a `\u` escape, joining surrogate pairs.
	fn append_unit(&mut self, unit: u16) {
		match unit {
			0xd800..=0xdbff => {
				if self.open_string().high_surrogate.is_some() {
					self.append_string("");
				}
				self.open_string().high_surrogate = Some(unit);
			}
			0xdc00..=0xdfff => match self.open_string().high_surrogate.take() {
				Some(high) => self.append_string(&String::from_utf16_lossy(&[high, unit])),
				None => self.append_string("\u{fffd}"),
			},
			_ => self.append_string(&String::from_utf16_lossy(&[unit])),
		}
	}

	/// Append `text` to the open string, after a U+FFFD for a high surrogate that did not get its low half.
	fn append_string(&mut self, text: &str) {
		let open = self.string.as_mut().expect("a string is open");
		let text = match open.high_surrogate.take() {
			Some(_) => format!("\u{fffd}{text}"),
			None => text.to_string(),
		};
		if text.is_empty() {
			return;
		}
		open.text.push_str(&text);
		if open.target != Target::Value {
			return;
		}
		if open.slot.is_some() {
			self.dirty = true;
		}
		let Some(path) = &open.path else {
			return;
		};
		// Extend this string's newest change while nothing came after it; otherwise report an append.
		if let Some(change) = open.change
			&& change + 1 == self.changes.len()
		{
			match &mut self.changes[change] {
				JsonChange::Set {
					value: Value::String(value),
					..
				} => value.push_str(&text),
				JsonChange::Append { text: appended, .. } => appended.push_str(&text),
				JsonChange::Set { .. } => unreachable!("a string's change sets a string"),
			}
			return;
		}
		self.changes.push(JsonChange::Append {
			path: path.clone(),
			text,
		});
		open.change = Some(self.changes.len() - 1);
	}

	fn end_string(&mut self) {
		if self.open_string().high_surrogate.is_some() {
			self.append_string("");
		}
		let open = self.string.take().expect("a string is open");
		if open.target == Target::Key {
			if let Some(frame) = self.stack.last_mut() {
				frame.key = Some(open.text);
				frame.phase = Phase::Colon;
			}
			return;
		}
		self.place(open.slot.as_ref(), Value::String(open.text));
		self.value_ended();
	}

	fn end_scalar(&mut self) {
		let token = std::mem::take(&mut self.scalar);
		let slot = self.scalar_slot.take();
		self.scalar_length = 0;
		if let Some(value) = scalar_value(&token) {
			let path = self.reported_path(slot.as_ref());
			if let Some(path) = path {
				self.changes.push(JsonChange::Set {
					path,
					value: value.clone(),
				});
			}
			self.place(slot.as_ref(), value);
		}
		self.value_ended();
	}

	/// What the unterminated value at the end of the text adds to a snapshot's cost, when it shows.
	fn pending_cost(&self) -> Option<usize> {
		if let Some(open) = &self.string {
			return (open.target == Target::Value && open.slot.is_some())
				.then(|| 1 + open.text.len() / STRING_BYTES_PER_COST);
		}
		self.pending_scalar().map(|_| 1)
	}

	/// The unterminated value at the end of the text and its slot, if it shows.
	fn pending(&self) -> Option<(Slot, Value)> {
		if let Some(open) = &self.string {
			return open.slot.clone().map(|slot| (slot, Value::String(open.text.clone())));
		}
		self.pending_scalar()
	}

	/// The value of the unterminated scalar at the end of the text, if it shows.
	fn pending_scalar(&self) -> Option<(Slot, Value)> {
		let slot = self.scalar_slot.as_ref()?;
		if self.scalar.is_empty() || self.scalar_length > MAX_PENDING_SCALAR_LENGTH {
			return None;
		}
		pending_scalar_value(&self.scalar).map(|value| (slot.clone(), value))
	}

	fn open_container(&mut self, container: Value) {
		let slot = self.value_slot();
		let path = self.reported_path(slot.as_ref());
		let is_array = container.is_array();
		self.stack.push(Frame {
			container,
			reports_inside: path.as_ref().is_some_and(|path| path.len() < MAX_REPORTED_DEPTH),
			path: path.clone(),
			slot,
			cost: 1,
			key: None,
			phase: if is_array { Phase::Value } else { Phase::Key },
		});
		self.path_cost += 1;
		self.dirty = true;
		// A fresh container: the working one keeps changing, and a change never does.
		if let Some(path) = path {
			let value = if is_array {
				Value::Array(Vec::new())
			} else {
				Value::Object(Map::new())
			};
			self.changes.push(JsonChange::Set { path, value });
		}
	}

	fn close_container(&mut self) {
		// A stray closer outside any container is ignored.
		let Some(frame) = self.stack.pop() else {
			return;
		};
		self.path_cost -= frame.cost;
		self.dirty = true;
		// Too deep to report inside: the whole value, final now that it is closed.
		if let Some(path) = &frame.path
			&& !frame.reports_inside
		{
			self.changes.push(JsonChange::Set {
				path: path.clone(),
				value: clone_deep(&frame.container),
			});
		}
		match &frame.slot {
			Some(Slot::Root) => self.root = Some(Rc::new(frame.container)),
			Some(slot) => {
				let parent = self
					.stack
					.last_mut()
					.expect("a member or element slot has an open container");
				parent.cost += frame.cost;
				self.path_cost += frame.cost;
				set_slot(&mut parent.container, slot, frame.container);
			}
			None => drop_deep(frame.container),
		}
		self.value_ended();
	}

	/// A value in the innermost container, or the root value when there is none, has ended.
	fn value_ended(&mut self) {
		match self.stack.last_mut() {
			// Text after the root value is ignored, but only once a root value exists.
			None => self.done = self.root.is_some(),
			Some(frame) => frame.phase = Phase::Comma,
		}
	}

	/// Set the value at `slot` in the open path; nothing for a value in an invalid position.
	fn place(&mut self, slot: Option<&Slot>, value: Value) {
		let Some(slot) = slot else {
			return;
		};
		self.dirty = true;
		if *slot == Slot::Root {
			self.root = Some(Rc::new(value));
			return;
		}
		let cost = match &value {
			Value::String(text) => 1 + text.len() / STRING_BYTES_PER_COST,
			_ => 1,
		};
		let frame = self
			.stack
			.last_mut()
			.expect("a member or element slot has an open container");
		frame.cost += cost;
		self.path_cost += cost;
		set_slot(&mut frame.container, slot, value);
	}

	/// A fresh snapshot: copies of the open containers, innermost first, each holding the copy of its open child.
	fn copy_open_path(&self, pending: Option<(Slot, Value)>) -> Option<Rc<Value>> {
		if self.stack.is_empty() {
			return match pending {
				Some((Slot::Root, value)) => Some(Rc::new(value)),
				_ => self.root.clone(),
			};
		}
		let mut child: Option<(Value, Option<Slot>)> = None;
		let mut pending = pending;
		for frame in self.stack.iter().rev() {
			let mut copy = clone_deep(&frame.container);
			if let Some((slot, value)) = pending.take() {
				set_slot(&mut copy, &slot, value);
			}
			if let Some((value, Some(slot))) = child.take() {
				set_slot(&mut copy, &slot, value);
			}
			child = Some((copy, frame.slot.clone()));
		}
		// The outermost open container is the root unless it sits in an invalid position.
		match child {
			Some((value, Some(Slot::Root))) => Some(Rc::new(value)),
			Some((value, _)) => {
				drop_deep(value);
				self.root.clone()
			}
			None => self.root.clone(),
		}
	}
}

impl Drop for StreamingJsonParser {
	fn drop(&mut self) {
		for frame in self.stack.drain(..) {
			drop_deep(frame.container);
		}
		for value in [self.root.take(), self.snapshot.take()].into_iter().flatten() {
			if let Ok(value) = Rc::try_unwrap(value) {
				drop_deep(value);
			}
		}
	}
}

/// `ToolCallArgumentsStream::replace`'s result.
#[derive(Clone, Debug, PartialEq)]
pub struct ReplacedArguments {
	pub delta: Option<String>,
	pub arguments: Rc<JsonObject>,
}

/// The arguments of one streaming tool call: the raw text, for the one full parse when the call ends, and a
/// `StreamingJsonParser` for the partial arguments in between. Providers keep one per call in their own stream state,
/// never on the content block, so the raw text never reaches partial messages or transcripts.
#[derive(Default)]
pub struct ToolCallArgumentsStream {
	text: String,
	parser: StreamingJsonParser,
	snapshot: Option<Rc<Value>>,
	arguments: Rc<JsonObject>,
}

impl ToolCallArgumentsStream {
	pub fn new() -> Self {
		Self::default()
	}

	/// The raw argument text received so far.
	pub fn text(&self) -> &str {
		&self.text
	}

	/// Append a piece of argument text; returns the partial arguments, a new `Rc` whenever they changed.
	pub fn append(&mut self, delta: &str) -> Rc<JsonObject> {
		self.text.push_str(delta);
		self.parser.append(delta);
		self.partial()
	}

	/// Take the provider's complete argument text. When it continues the text so far, returns the missing piece as
	/// `delta`; otherwise starts over with `text`. Either way returns the partial arguments of `text`.
	pub fn replace(&mut self, text: &str) -> ReplacedArguments {
		if let Some(delta) = text.strip_prefix(self.text.as_str()) {
			let arguments = if delta.is_empty() {
				self.partial()
			} else {
				self.append(delta)
			};
			return ReplacedArguments {
				delta: Some(delta.to_string()),
				arguments,
			};
		}
		self.text = String::new();
		self.parser = StreamingJsonParser::new();
		ReplacedArguments {
			delta: None,
			arguments: self.append(text),
		}
	}

	/// The final arguments: one full parse of the complete text, which reports malformed arguments.
	pub fn finish(&self) -> ParsedToolCallArguments {
		parse_tool_call_arguments(Some(&self.text))
	}

	fn partial(&mut self) -> Rc<JsonObject> {
		let snapshot = self.parser.snapshot();
		let unchanged = match (&snapshot, &self.snapshot) {
			(None, None) => true,
			(Some(snapshot), Some(previous)) => {
				Rc::ptr_eq(snapshot, previous)
					|| (!snapshot.is_object() && !snapshot.is_array() && snapshot == previous)
			}
			_ => false,
		};
		if unchanged {
			return self.arguments.clone();
		}
		// Tool arguments are an object; any other partial value shows as no arguments yet.
		self.arguments = Rc::new(match snapshot.as_deref() {
			Some(value @ Value::Object(_)) => match clone_deep(value) {
				Value::Object(arguments) => arguments,
				_ => unreachable!("a copy of an object is an object"),
			},
			_ => Map::new(),
		});
		self.snapshot = snapshot;
		self.arguments.clone()
	}
}

fn simple_escape(byte: u8) -> Option<&'static str> {
	Some(match byte {
		b'"' => "\"",
		b'\\' => "\\",
		b'/' => "/",
		b'b' => "\u{8}",
		b'f' => "\u{c}",
		b'n' => "\n",
		b'r' => "\r",
		b't' => "\t",
		_ => return None,
	})
}

fn is_delimiter(byte: u8) -> bool {
	matches!(
		byte,
		b' ' | b'\t' | b'\n' | b'\r' | b',' | b':' | b'{' | b'}' | b'[' | b']' | b'"'
	)
}

/// The value of a complete number or literal token; `None` for anything else, which is dropped.
fn scalar_value(token: &str) -> Option<Value> {
	for (literal, value) in LITERALS {
		if token == literal {
			return Some(value);
		}
	}
	NUMBER.is_match(token).then(|| js_number(token))
}

/// The value an unterminated number or literal token shows.
fn pending_scalar_value(token: &str) -> Option<Value> {
	for (literal, value) in LITERALS {
		if literal.starts_with(token) {
			return Some(value);
		}
	}
	if NUMBER.is_match(token) {
		return Some(js_number(token));
	}
	NUMBER_WITHOUT_EXPONENT_DIGITS
		.captures(token)
		.map(|mantissa| js_number(&mantissa[1]))
}

/// `Number(token)` of a token that matches the JSON number grammar.
fn js_number(token: &str) -> Value {
	number_value(token.parse().expect("a JSON number parses as f64"))
}

fn set_slot(container: &mut Value, slot: &Slot, value: Value) {
	match (container, slot) {
		(Value::Array(elements), Slot::Element(index)) => {
			if *index == elements.len() {
				elements.push(value);
			} else {
				elements[*index] = value;
			}
		}
		(Value::Object(members), Slot::Member(key)) => set_js_member(members, key.clone(), value),
		_ => {}
	}
}

/// `value.clone()` without recursion, so arbitrarily deep values do not overflow the stack.
fn clone_deep(value: &Value) -> Value {
	enum Task<'a> {
		Visit(&'a Value),
		Array(usize),
		Object(&'a JsonObject),
	}
	let mut tasks = vec![Task::Visit(value)];
	let mut built: Vec<Value> = Vec::new();
	while let Some(task) = tasks.pop() {
		match task {
			Task::Visit(Value::Array(elements)) => {
				tasks.push(Task::Array(elements.len()));
				tasks.extend(elements.iter().rev().map(Task::Visit));
			}
			Task::Visit(Value::Object(members)) => {
				tasks.push(Task::Object(members));
				tasks.extend(members.values().rev().map(Task::Visit));
			}
			Task::Visit(scalar) => built.push(scalar.clone()),
			Task::Array(length) => {
				let elements = built.split_off(built.len() - length);
				built.push(Value::Array(elements));
			}
			Task::Object(members) => {
				let values = built.split_off(built.len() - members.len());
				built.push(Value::Object(members.keys().cloned().zip(values).collect()));
			}
		}
	}
	built.pop().expect("one value was built")
}

/// Drops `value` without recursion, so arbitrarily deep values do not overflow the stack.
fn drop_deep(value: Value) {
	let mut values = vec![value];
	while let Some(value) = values.pop() {
		match value {
			Value::Array(elements) => values.extend(elements),
			Value::Object(members) => values.extend(members.into_iter().map(|(_, value)| value)),
			_ => {}
		}
	}
}
