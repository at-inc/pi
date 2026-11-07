//! Port of `packages/ai/src/utils/json-parse.ts`.
//! PORT-STATUS: ported
//!
//! TS calls `JSON.parse`, whose error messages end up in `ToolCall.argumentsError`, and the `partial-json` package.
//! Both are ported here: [`json_parse`] follows V8's JSON parser, including its error messages, positions in UTF-16
//! code units and object key order (array-index keys first, ascending); `partial_json_parse` follows
//! `partial-json`'s `parse` with every partial type allowed. Two values cannot be represented: a lone surrogate
//! escape decodes to U+FFFD, and the `Infinity`, `-Infinity` and `NaN` that `partial-json` accepts become `null`,
//! which is what `JSON.stringify` writes for them (`parse_streaming_json` of a root `NaN` therefore returns `null`,
//! where TS returns the number).
//!
//! Other differences, all on values a `serde_json::Value` or a Rust string cannot hold:
//! - A number that overflows (`1e400`) is `null`; TS has `Infinity`. Both serialize as `null`, but a tool argument
//!   validated against `type: "number"` passes in TS and fails here.
//! - V8 cuts the unexpected token and the error context by UTF-16 code unit, so a non-BMP character there leaves a
//!   lone surrogate in the TS message; here that half is U+FFFD.
//! - partial-json assigns members with `obj[key] = value`, so an object value under `__proto__` becomes the TS
//!   result's prototype and the member is not an own key; here the member is dropped, which serializes the same.

use pi_host::js::number_to_string;
use pi_host::{PiError, PiResult};
use serde_json::{Map, Number, Value};

use super::text::js_trim;
use crate::types::JsonObject;

const VALID_JSON_ESCAPES: [char; 8] = ['"', '\\', '/', 'b', 'f', 'n', 'r', 't'];

fn is_control_character(ch: char) -> bool {
	ch <= '\u{1f}'
}

fn escape_control_character(ch: char) -> String {
	match ch {
		'\u{8}' => "\\b".into(),
		'\u{c}' => "\\f".into(),
		'\n' => "\\n".into(),
		'\r' => "\\r".into(),
		'\t' => "\\t".into(),
		_ => format!("\\u{:04x}", u32::from(ch)),
	}
}

/// Repairs malformed JSON string literals by:
/// - escaping raw control characters inside strings
/// - doubling backslashes before invalid escape characters
pub fn repair_json(json: &str) -> String {
	let chars: Vec<char> = json.chars().collect();
	let mut repaired = String::with_capacity(json.len());
	let mut in_string = false;
	let mut index = 0;

	while index < chars.len() {
		let ch = chars[index];
		index += 1;

		if !in_string {
			repaired.push(ch);
			if ch == '"' {
				in_string = true;
			}
			continue;
		}

		if ch == '"' {
			repaired.push(ch);
			in_string = false;
			continue;
		}

		if ch == '\\' {
			let Some(&next_char) = chars.get(index) else {
				repaired.push_str("\\\\");
				continue;
			};

			if next_char == 'u'
				&& let Some(digits) = chars
					.get(index + 1..index + 5)
					.filter(|digits| digits.iter().all(char::is_ascii_hexdigit))
			{
				repaired.push_str("\\u");
				repaired.extend(digits);
				index += 5;
				continue;
			}

			if VALID_JSON_ESCAPES.contains(&next_char) {
				repaired.push('\\');
				repaired.push(next_char);
				index += 1;
				continue;
			}

			repaired.push_str("\\\\");
			continue;
		}

		if is_control_character(ch) {
			repaired.push_str(&escape_control_character(ch));
		} else {
			repaired.push(ch);
		}
	}

	repaired
}

pub fn parse_json_with_repair(json: &str) -> PiResult<Value> {
	match json_parse(json) {
		Ok(value) => Ok(value),
		Err(error) => {
			let repaired_json = repair_json(json);
			if repaired_json != json {
				return json_parse(&repaired_json);
			}
			Err(error)
		}
	}
}

/// `Pick<ToolCall, "arguments" | "argumentsError">`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ParsedToolCallArguments {
	pub arguments: JsonObject,
	pub arguments_error: Option<String>,
}

/// Parses the model's complete arguments text of a finished tool call.
///
/// Unlike [`parse_streaming_json`], this never keeps a partially parsed prefix: once the call is
/// finished, text that is not a valid JSON object means the model sent malformed arguments. In that
/// case `arguments` is `{}` and `arguments_error` describes the problem, including the text the
/// model sent, so tool validation can report it back to the model.
pub fn parse_tool_call_arguments(json: Option<&str>) -> ParsedToolCallArguments {
	let Some(json) = json.filter(|json| !js_trim(json).is_empty()) else {
		return ParsedToolCallArguments::default();
	};
	let message = match parse_json_with_repair(json) {
		Ok(Value::Object(arguments)) => {
			return ParsedToolCallArguments {
				arguments,
				arguments_error: None,
			};
		}
		Ok(_) => "Tool arguments must be a JSON object".to_string(),
		Err(error) => error.message,
	};
	ParsedToolCallArguments {
		arguments: JsonObject::new(),
		arguments_error: Some(format!("{message}\n\nReceived arguments:\n{json}")),
	}
}

/// Attempts to parse potentially incomplete JSON during streaming.
/// Always returns a valid value, even if the JSON is incomplete: `{}` if parsing fails.
pub fn parse_streaming_json(partial_json: Option<&str>) -> Value {
	let Some(partial_json) = partial_json.filter(|json| !js_trim(json).is_empty()) else {
		return Value::Object(Map::new());
	};

	if let Ok(value) = parse_json_with_repair(partial_json) {
		return value;
	}
	let partial = partial_json_parse(partial_json).or_else(|()| partial_json_parse(&repair_json(partial_json)));
	match partial {
		// `partialParse(...) ?? {}` keeps a root `NaN` or `Infinity`, which reads as `null` here; only `null` itself
		// becomes `{}`. partial-json trims the text with `String.prototype.trim` before it reads the root value.
		Ok(Value::Null) if !js_trim(partial_json).starts_with('n') => Value::Null,
		Ok(Value::Null) | Err(()) => Value::Object(Map::new()),
		Ok(value) => value,
	}
}

/// `JSON.parse(text)` as V8 implements it: the same values, the same object key order, and a `SyntaxError` with
/// the same message.
pub fn json_parse(text: &str) -> PiResult<Value> {
	let units: Vec<u16> = text.encode_utf16().collect();
	JsonParser {
		source: &units,
		cursor: 0,
	}
	.parse()
}

/// The JSON number `value` as a `Value`: an integer when it is one, so it compares equal to `json!` literals.
pub(crate) fn number_value(value: f64) -> Value {
	if value.fract() == 0.0 && value.abs() < 9_007_199_254_740_992.0 && !(value == 0.0 && value.is_sign_negative()) {
		return Value::from(value as i64);
	}
	Number::from_f64(value).map_or(Value::Null, Value::Number)
}

/// Sets `key` the way a JS object does: an existing key keeps its position, and an array-index key goes before the
/// other keys, in ascending order.
pub(crate) fn set_js_member(members: &mut JsonObject, key: String, value: Value) {
	if members.contains_key(&key) {
		members.insert(key, value);
		return;
	}
	let Some(index) = array_index(&key) else {
		members.insert(key, value);
		return;
	};
	let position = members
		.keys()
		.position(|existing| array_index(existing).is_none_or(|existing| existing > index))
		.unwrap_or(members.len());
	members.shift_insert(position, key, value);
}

fn array_index(key: &str) -> Option<u32> {
	if key.is_empty() || key.len() > 10 || !key.bytes().all(|byte| byte.is_ascii_digit()) {
		return None;
	}
	if key.len() > 1 && key.starts_with('0') {
		return None;
	}
	key.parse::<u64>()
		.ok()
		.filter(|index| *index < 4_294_967_295)
		.map(|index| index as u32)
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Token {
	Eos,
	String,
	Number,
	LBrace,
	RBrace,
	LBrack,
	RBrack,
	Colon,
	Comma,
	True,
	False,
	Null,
	Whitespace,
	Illegal,
}

fn token_of(unit: u16) -> Token {
	match unit {
		0x22 => Token::String,
		0x2d | 0x30..=0x39 => Token::Number,
		0x7b => Token::LBrace,
		0x7d => Token::RBrace,
		0x5b => Token::LBrack,
		0x5d => Token::RBrack,
		0x3a => Token::Colon,
		0x2c => Token::Comma,
		0x74 => Token::True,
		0x66 => Token::False,
		0x6e => Token::Null,
		0x20 | 0x09 | 0x0a | 0x0d => Token::Whitespace,
		_ => Token::Illegal,
	}
}

const EXPECTED_PROP_NAME_OR_RBRACE: &str = "Expected property name or '}' in JSON";
const EXPECTED_COMMA_OR_RBRACK: &str = "Expected ',' or ']' after array element in JSON";
const EXPECTED_COMMA_OR_RBRACE: &str = "Expected ',' or '}' after property value in JSON";
const EXPECTED_COLON_AFTER_PROPERTY_NAME: &str = "Expected ':' after property name in JSON";
const EXPECTED_DOUBLE_QUOTED_PROPERTY_NAME: &str = "Expected double-quoted property name in JSON";
const UNEXPECTED_NON_WHITESPACE: &str = "Unexpected non-whitespace character after JSON";
const UNTERMINATED_STRING: &str = "Unterminated string in JSON";
const BAD_CONTROL_CHARACTER: &str = "Bad control character in string literal in JSON";
const BAD_ESCAPED_CHARACTER: &str = "Bad escaped character in JSON";
const BAD_UNICODE_ESCAPE: &str = "Bad Unicode escape in JSON";
const NO_NUMBER_AFTER_MINUS_SIGN: &str = "No number after minus sign in JSON";
const EXPONENT_PART_MISSING_NUMBER: &str = "Exponent part is missing a number in JSON";
const UNTERMINATED_FRACTIONAL_NUMBER: &str = "Unterminated fractional number in JSON";
const MAX_CONTEXT_CHARACTERS: usize = 10;

enum Open {
	Object { members: JsonObject, key: String },
	Array(Vec<Value>),
}

struct JsonParser<'a> {
	source: &'a [u16],
	cursor: usize,
}

impl JsonParser<'_> {
	fn parse(mut self) -> PiResult<Value> {
		let value = self.parse_value()?;
		self.skip_whitespace();
		if self.peek() != Token::Eos {
			return Err(self.error(Some(UNEXPECTED_NON_WHITESPACE)));
		}
		Ok(value)
	}

	fn parse_value(&mut self) -> PiResult<Value> {
		let mut stack: Vec<Open> = Vec::new();
		loop {
			self.skip_whitespace();
			let mut value = match self.peek() {
				Token::String => Value::String(self.scan_string()?),
				Token::Number => self.scan_number()?,
				Token::LBrace => {
					self.cursor += 1;
					if self.check(Token::RBrace) {
						Value::Object(Map::new())
					} else {
						let key = self.scan_property_key(EXPECTED_PROP_NAME_OR_RBRACE)?;
						stack.push(Open::Object {
							members: Map::new(),
							key,
						});
						continue;
					}
				}
				Token::LBrack => {
					self.cursor += 1;
					if self.check(Token::RBrack) {
						Value::Array(Vec::new())
					} else {
						stack.push(Open::Array(Vec::new()));
						continue;
					}
				}
				Token::True => {
					self.scan_literal("true")?;
					Value::Bool(true)
				}
				Token::False => {
					self.scan_literal("false")?;
					Value::Bool(false)
				}
				Token::Null => {
					self.scan_literal("null")?;
					Value::Null
				}
				_ => return Err(self.error(None)),
			};

			loop {
				match stack.last_mut() {
					None => return Ok(value),
					Some(Open::Object { members, key }) => {
						set_js_member(members, std::mem::take(key), value);
						if self.check(Token::Comma) {
							*key = self.scan_property_key(EXPECTED_DOUBLE_QUOTED_PROPERTY_NAME)?;
							break;
						}
						if self.peek() != Token::RBrace {
							return Err(self.error(Some(EXPECTED_COMMA_OR_RBRACE)));
						}
						self.cursor += 1;
						let Some(Open::Object { members, .. }) = stack.pop() else {
							unreachable!("the open container is an object");
						};
						value = Value::Object(members);
					}
					Some(Open::Array(elements)) => {
						elements.push(value);
						if self.check(Token::Comma) {
							break;
						}
						if self.peek() != Token::RBrack {
							return Err(self.error(Some(EXPECTED_COMMA_OR_RBRACK)));
						}
						self.cursor += 1;
						let Some(Open::Array(elements)) = stack.pop() else {
							unreachable!("the open container is an array");
						};
						value = Value::Array(elements);
					}
				}
			}
		}
	}

	fn scan_property_key(&mut self, message: &'static str) -> PiResult<String> {
		self.skip_whitespace();
		if self.peek() != Token::String {
			return Err(self.error(Some(message)));
		}
		let key = self.scan_string()?;
		self.skip_whitespace();
		if self.peek() != Token::Colon {
			return Err(self.error(Some(EXPECTED_COLON_AFTER_PROPERTY_NAME)));
		}
		self.cursor += 1;
		Ok(key)
	}

	fn peek(&self) -> Token {
		self.source.get(self.cursor).map_or(Token::Eos, |unit| token_of(*unit))
	}

	fn check(&mut self, token: Token) -> bool {
		self.skip_whitespace();
		if self.peek() != token {
			return false;
		}
		self.cursor += 1;
		true
	}

	fn skip_whitespace(&mut self) {
		while self.peek() == Token::Whitespace {
			self.cursor += 1;
		}
	}

	fn scan_literal(&mut self, literal: &str) -> PiResult<()> {
		for expected in literal.encode_utf16() {
			if self.source.get(self.cursor) != Some(&expected) {
				return Err(self.error(None));
			}
			self.cursor += 1;
		}
		Ok(())
	}

	fn scan_string(&mut self) -> PiResult<String> {
		self.cursor += 1;
		let mut units: Vec<u16> = Vec::new();
		loop {
			let Some(&unit) = self.source.get(self.cursor) else {
				return Err(self.error(Some(UNTERMINATED_STRING)));
			};
			match unit {
				0x22 => {
					self.cursor += 1;
					return Ok(String::from_utf16_lossy(&units));
				}
				0x5c => {
					self.cursor += 1;
					let Some(&escape) = self.source.get(self.cursor) else {
						return Err(self.error(None));
					};
					let decoded = match escape {
						0x22 | 0x5c | 0x2f => escape,
						0x62 => 0x08,
						0x66 => 0x0c,
						0x6e => 0x0a,
						0x72 => 0x0d,
						0x74 => 0x09,
						0x75 => self.scan_unicode_escape()?,
						0x100.. => return Err(self.error(None)),
						_ => return Err(self.error(Some(BAD_ESCAPED_CHARACTER))),
					};
					units.push(decoded);
					self.cursor += 1;
				}
				0x00..=0x1f => return Err(self.error(Some(BAD_CONTROL_CHARACTER))),
				_ => {
					units.push(unit);
					self.cursor += 1;
				}
			}
		}
	}

	/// Reads the four hex digits after `\u`; leaves the cursor on the last one.
	fn scan_unicode_escape(&mut self) -> PiResult<u16> {
		let mut value: u16 = 0;
		for _ in 0..4 {
			self.cursor += 1;
			let digit = self
				.source
				.get(self.cursor)
				.and_then(|unit| char::from_u32(u32::from(*unit)))
				.and_then(|ch| ch.to_digit(16));
			let Some(digit) = digit else {
				return Err(self.error(Some(BAD_UNICODE_ESCAPE)));
			};
			value = value * 16 + digit as u16;
		}
		Ok(value)
	}

	fn is_digit_at(&self, index: usize) -> bool {
		self.source.get(index).is_some_and(|unit| (0x30..=0x39).contains(unit))
	}

	fn skip_digits(&mut self) {
		while self.is_digit_at(self.cursor) {
			self.cursor += 1;
		}
	}

	fn scan_number(&mut self) -> PiResult<Value> {
		let start = self.cursor;
		if self.source[self.cursor] == 0x2d {
			self.cursor += 1;
		}
		if self.source.get(self.cursor) == Some(&0x30) {
			self.cursor += 1;
			if self.is_digit_at(self.cursor) {
				return Err(self.error(None));
			}
		} else {
			if !self.is_digit_at(self.cursor) {
				return Err(self.error(Some(NO_NUMBER_AFTER_MINUS_SIGN)));
			}
			self.skip_digits();
		}
		if self.source.get(self.cursor) == Some(&0x2e) {
			self.cursor += 1;
			if !self.is_digit_at(self.cursor) {
				return Err(self.error(Some(UNTERMINATED_FRACTIONAL_NUMBER)));
			}
			self.skip_digits();
		}
		if matches!(self.source.get(self.cursor), Some(0x65 | 0x45)) {
			self.cursor += 1;
			if matches!(self.source.get(self.cursor), Some(0x2b | 0x2d)) {
				self.cursor += 1;
			}
			if !self.is_digit_at(self.cursor) {
				return Err(self.error(Some(EXPONENT_PART_MISSING_NUMBER)));
			}
			self.skip_digits();
		}
		let text = String::from_utf16_lossy(&self.source[start..self.cursor]);
		let value: f64 = text.parse().expect("a scanned JSON number parses as f64");
		Ok(number_value(value))
	}

	/// The `SyntaxError` V8 throws at the cursor: `message` with the position, or V8's message for the unexpected
	/// token.
	fn error(&self, message: Option<&str>) -> PiError {
		let position = self.cursor;
		let located = |message: &str| {
			let (line, column) = self.line_and_column();
			format!("{message} at position {position} (line {line} column {column})")
		};
		let message = match (message, self.peek()) {
			(Some(message), _) => located(message),
			(None, Token::Eos) => "Unexpected end of JSON input".to_string(),
			(None, Token::Number) => located("Unexpected number in JSON"),
			(None, Token::String) => located("Unexpected string in JSON"),
			(None, _) => self.unexpected_token_message(position),
		};
		PiError::named("SyntaxError", message)
	}

	fn unexpected_token_message(&self, position: usize) -> String {
		let source = String::from_utf16_lossy(self.source);
		if matches!(source.as_str(), "[object Object]" | "undefined" | "Infinity" | "NaN") {
			return format!("\"{source}\" is not valid JSON");
		}
		let token = String::from_utf16_lossy(&self.source[position..position + 1]);
		let length = self.source.len();
		if length < MAX_CONTEXT_CHARACTERS * 2 + 1 {
			return format!("Unexpected token '{token}', \"{source}\" is not valid JSON");
		}
		let excerpt = |start: usize, end: usize| String::from_utf16_lossy(&self.source[start..end.min(length)]);
		if position < MAX_CONTEXT_CHARACTERS {
			let context = excerpt(0, position + MAX_CONTEXT_CHARACTERS);
			return format!("Unexpected token '{token}', \"{context}\"... is not valid JSON");
		}
		let start = position - MAX_CONTEXT_CHARACTERS;
		if position < length - MAX_CONTEXT_CHARACTERS {
			let context = excerpt(start, position + MAX_CONTEXT_CHARACTERS);
			return format!("Unexpected token '{token}', ...\"{context}\"... is not valid JSON");
		}
		format!(
			"Unexpected token '{token}', ...\"{}\" is not valid JSON",
			excerpt(start, length)
		)
	}

	/// The 1-based line and column of the cursor. JSON line terminators are `\n`, `\r` and `\r\n`.
	fn line_and_column(&self) -> (usize, usize) {
		let mut line = 1;
		let mut line_start = 0;
		let mut index = 0;
		while index < self.cursor {
			if self.source[index] == 0x0d && index + 1 < self.cursor && self.source[index + 1] == 0x0a {
				index += 1;
			}
			if matches!(self.source[index], 0x0a | 0x0d) {
				line += 1;
				line_start = index + 1;
			}
			index += 1;
		}
		(line, 1 + self.cursor - line_start)
	}
}

/// `parse` from `partial-json` with `Allow.ALL`, which `parseStreamingJson` uses. Any error it throws is `Err`.
fn partial_json_parse(json: &str) -> Result<Value, ()> {
	let trimmed = js_trim(json);
	if trimmed.is_empty() {
		return Err(());
	}
	let units: Vec<u16> = trimmed.encode_utf16().collect();
	PartialJsonParser {
		source: &units,
		index: 0,
	}
	.parse_any()
}

struct PartialJsonParser<'a> {
	source: &'a [u16],
	index: usize,
}

const PARTIAL_LITERALS: [(&str, Value, usize); 6] = [
	("null", Value::Null, 0),
	("true", Value::Bool(true), 0),
	("false", Value::Bool(false), 0),
	("Infinity", Value::Null, 0),
	("-Infinity", Value::Null, 1),
	("NaN", Value::Null, 0),
];

impl PartialJsonParser<'_> {
	fn is_at(&self, index: usize, ch: u8) -> bool {
		self.source.get(index) == Some(&u16::from(ch))
	}

	/// `jsonString.substring(start, end)`: clamped to the string and swapped when `start > end`.
	fn substring(&self, start: isize, end: isize) -> String {
		let clamp = |value: isize| value.clamp(0, self.source.len() as isize) as usize;
		let (start, end) = (clamp(start), clamp(end));
		String::from_utf16_lossy(&self.source[start.min(end)..start.max(end)])
	}

	fn last_index_of(&self, ch: u8) -> isize {
		self.source
			.iter()
			.rposition(|unit| *unit == u16::from(ch))
			.map_or(-1, |index| index as isize)
	}

	fn parse_any(&mut self) -> Result<Value, ()> {
		self.skip_blank();
		if self.index >= self.source.len() {
			return Err(());
		}
		if self.is_at(self.index, b'"') {
			return self.parse_str();
		}
		if self.is_at(self.index, b'{') {
			return Ok(self.parse_obj());
		}
		if self.is_at(self.index, b'[') {
			return Ok(self.parse_arr());
		}
		for (literal, value, min_remaining) in PARTIAL_LITERALS {
			if self.matches_literal(literal, min_remaining) {
				self.index += literal.len();
				return Ok(value);
			}
		}
		self.parse_num()
	}

	/// `substring(index, index + n) === literal`, or the rest of the text is a prefix of `literal` shorter than it
	/// and longer than `min_remaining`.
	fn matches_literal(&self, literal: &str, min_remaining: usize) -> bool {
		let literal: Vec<u16> = literal.encode_utf16().collect();
		let rest = &self.source[self.index..];
		rest.starts_with(&literal)
			|| (min_remaining < rest.len() && rest.len() < literal.len() && literal.starts_with(rest))
	}

	fn parse_str(&mut self) -> Result<Value, ()> {
		let start = self.index as isize;
		let mut escape = false;
		self.index += 1;
		while self.index < self.source.len()
			&& (!self.is_at(self.index, b'"') || (escape && self.is_at(self.index - 1, b'\\')))
		{
			escape = self.is_at(self.index, b'\\') && !escape;
			self.index += 1;
		}
		let escape = isize::from(escape);
		if self.is_at(self.index, b'"') {
			self.index += 1;
			return partial_json_value(&self.substring(start, self.index as isize - escape));
		}
		let partial = format!("{}\"", self.substring(start, self.index as isize - escape));
		partial_json_value(&partial).or_else(|()| {
			let cut = format!("{}\"", self.substring(start, self.last_index_of(b'\\')));
			partial_json_value(&cut)
		})
	}

	fn parse_obj(&mut self) -> Value {
		self.index += 1;
		self.skip_blank();
		let mut members = Map::new();
		while !self.is_at(self.index, b'}') {
			self.skip_blank();
			if self.index >= self.source.len() {
				return Value::Object(members);
			}
			let Ok(key) = self.parse_str() else {
				return Value::Object(members);
			};
			self.skip_blank();
			self.index += 1;
			let Ok(value) = self.parse_any() else {
				return Value::Object(members);
			};
			let key = property_key(&key);
			if key != "__proto__" {
				set_js_member(&mut members, key, value);
			}
			self.skip_blank();
			if self.is_at(self.index, b',') {
				self.index += 1;
			}
		}
		self.index += 1;
		Value::Object(members)
	}

	fn parse_arr(&mut self) -> Value {
		self.index += 1;
		let mut elements = Vec::new();
		while !self.is_at(self.index, b']') {
			let Ok(element) = self.parse_any() else {
				return Value::Array(elements);
			};
			elements.push(element);
			self.skip_blank();
			if self.is_at(self.index, b',') {
				self.index += 1;
			}
		}
		self.index += 1;
		Value::Array(elements)
	}

	fn parse_num(&mut self) -> Result<Value, ()> {
		if self.index == 0 {
			let whole = self.substring(0, self.source.len() as isize);
			if whole == "-" {
				return Err(());
			}
			return partial_json_value(&whole)
				.or_else(|()| partial_json_value(&self.substring(0, self.last_index_of(b'e'))));
		}
		let start = self.index as isize;
		if self.is_at(self.index, b'-') {
			self.index += 1;
		}
		while self
			.source
			.get(self.index)
			.is_some_and(|unit| !b",]}".map(u16::from).contains(unit))
		{
			self.index += 1;
		}
		let token = self.substring(start, self.index as isize);
		partial_json_value(&token).or_else(|()| {
			if token == "-" {
				return Err(());
			}
			partial_json_value(&self.substring(start, self.last_index_of(b'e')))
		})
	}

	fn skip_blank(&mut self) {
		while self
			.source
			.get(self.index)
			.is_some_and(|unit| b" \n\r\t".map(u16::from).contains(unit))
		{
			self.index += 1;
		}
	}
}

fn partial_json_value(text: &str) -> Result<Value, ()> {
	json_parse(text).map_err(|_| ())
}

/// `String(key)`, the property key a JS object assignment `obj[key] = value` uses.
fn property_key(key: &Value) -> String {
	match key {
		Value::String(key) => key.clone(),
		Value::Null => "null".into(),
		Value::Bool(value) => value.to_string(),
		Value::Number(number) => number_to_string(number.as_f64().unwrap_or(f64::NAN)),
		Value::Array(elements) => elements
			.iter()
			.map(|element| match element {
				Value::Null => String::new(),
				element => property_key(element),
			})
			.collect::<Vec<_>>()
			.join(","),
		Value::Object(_) => "[object Object]".into(),
	}
}
