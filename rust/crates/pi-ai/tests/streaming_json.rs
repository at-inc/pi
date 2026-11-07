//! Port of `packages/ai/test/streaming-json.test.ts`.
//!
//! Rust differences in the tests: pieces are split by characters, since a `&str` cannot end inside a surrogate pair;
//! the reference parser and expected values use this crate's `json_parse` (the V8 port) where TS uses `JSON.parse`; a
//! high surrogate whose low half has not arrived is held back instead of shown alone. The allocation and garbage
//! collection tests measure V8's heap and have no Rust counterpart; the cost tests keep the time and monotonic-preview
//! checks.

use std::rc::Rc;
use std::time::Instant;

use pi_ai::utils::json_parse::{json_parse, parse_streaming_json, parse_tool_call_arguments};
use pi_ai::utils::streaming_json::{JsonChange, JsonPathSegment, StreamingJsonParser, ToolCallArgumentsStream};
use pi_host::js::json_stringify;
use serde_json::{Map, Value, json};

// #9265: streamed tool-call arguments were re-parsed in full on every delta.

/// Seeded random numbers, so every run checks the same documents. Follows the TS double arithmetic exactly.
struct Random(f64);

impl Random {
	fn new(seed: u32) -> Self {
		Self(f64::from(seed))
	}

	fn next(&mut self) -> f64 {
		let product = self.0 * 1_103_515_245.0 + 12345.0;
		self.0 = ((product as i64) & 0x7fff_ffff) as f64;
		self.0 / f64::from(0x7fff_ffff)
	}

	fn index(&mut self, length: usize) -> usize {
		(self.next() * length as f64).floor() as usize
	}

	fn pick<T: Clone>(&mut self, items: &[T]) -> T {
		items[self.index(items.len())].clone()
	}
}

const STRING_PIECES: [&str; 17] = [
	"a", "b", "x y", " ", "\n", "\t", "\\", "\"", "/", "é", "😀", "\u{1}", "\u{1f}", "\u{2028}", "{", "]", ",",
];

fn random_string(rand: &mut Random) -> String {
	let length = rand.index(6);
	(0..length).map(|_| rand.pick(&STRING_PIECES)).collect()
}

fn random_scalar(rand: &mut Random) -> Value {
	match rand.index(10) {
		0 => Value::String(random_string(rand)),
		1 => json!((rand.next() * 2000.0).floor() - 1000.0),
		2 => json!(rand.next() * 100.0),
		3 => json!(1e21),
		4 => json!(-5e-8),
		5 => json!(-0.0),
		6 => json!(0),
		7 => json!(true),
		8 => json!(false),
		_ => Value::Null,
	}
}

fn random_value(rand: &mut Random, depth: usize) -> Value {
	let roll = rand.next();
	if depth > 3 || roll < 0.3 {
		return random_scalar(rand);
	}
	if roll < 0.65 {
		let mut members = Map::new();
		let count = rand.index(4);
		for _ in 0..count {
			let key = random_string(rand);
			members.insert(key, random_value(rand, depth + 1));
		}
		return Value::Object(members);
	}
	let count = rand.index(4);
	Value::Array((0..count).map(|_| random_value(rand, depth + 1)).collect())
}

/// JSON text of `value`, with random insignificant whitespace when `spaced`.
fn render(value: &Value, rand: &mut Random, spaced: bool) -> String {
	let space = |rand: &mut Random| {
		if spaced {
			rand.pick(&["", "", " ", "\n  ", "\t", "\r\n"]).to_string()
		} else {
			String::new()
		}
	};
	match value {
		Value::Array(items) => {
			let open = space(rand);
			let mut parts = Vec::new();
			for item in items {
				parts.push(render(item, rand, spaced));
			}
			let separator = format!("{},{}", space(rand), space(rand));
			let close = space(rand);
			format!("[{open}{}{close}]", parts.join(&separator))
		}
		Value::Object(members) => {
			let open = space(rand);
			let mut parts = Vec::new();
			for (key, item) in members {
				let key = json_stringify(key);
				let before = space(rand);
				let after = space(rand);
				parts.push(format!("{key}{before}:{after}{}", render(item, rand, spaced)));
			}
			let separator = format!("{},{}", space(rand), space(rand));
			let close = space(rand);
			format!("{{{open}{}{close}}}", parts.join(&separator))
		}
		// JSON.stringify(-0) is "0"; keep the sign so the text parses back to the same value.
		Value::Number(number) if number.as_f64() == Some(0.0) && number.as_f64().unwrap().is_sign_negative() => {
			"-0".into()
		}
		other => json_stringify(other),
	}
}

fn random_documents(seed: u32, count: usize, spaced: bool) -> Vec<String> {
	let mut rand = Random::new(seed);
	(0..count)
		.map(|_| {
			let value = if rand.next() < 0.8 {
				let path = random_string(&mut rand);
				let args = random_value(&mut rand, 0);
				json!({ "path": path, "args": args })
			} else {
				random_value(&mut rand, 0)
			};
			render(&value, &mut rand, spaced)
		})
		.collect()
}

/// Reference for `StreamingJsonParser` on a prefix of valid JSON (strings may also hold raw control characters and
/// invalid escapes): a plain recursive-descent reading of the whole prefix that follows the documented rules. It is
/// written independently of the incremental parser so the two check each other.
fn reference_snapshot(text: &str, pending_scalars: bool) -> Option<Value> {
	let chars: Vec<char> = text.chars().collect();
	let reference = Reference {
		text: &chars,
		pending_scalars,
	};
	reference.read_value(reference.skip_space(0)).map(|read| read.value)
}

struct Read {
	value: Value,
	complete: bool,
	end: usize,
}

struct Reference<'a> {
	text: &'a [char],
	pending_scalars: bool,
}

impl Reference<'_> {
	fn skip_space(&self, start: usize) -> usize {
		let mut index = start;
		while index < self.text.len() && " \t\n\r".contains(self.text[index]) {
			index += 1;
		}
		index
	}

	fn read_value(&self, start: usize) -> Option<Read> {
		match self.text.get(start)? {
			'"' => Some(self.read_string(start)),
			'{' => Some(self.read_object(start)),
			'[' => Some(self.read_array(start)),
			_ => self.read_scalar(start),
		}
	}

	fn read_string(&self, start: usize) -> Read {
		let text = self.text;
		let mut value = String::new();
		let mut index = start + 1;
		let partial = |value: String| Read {
			value: Value::String(value),
			complete: false,
			end: text.len(),
		};
		while index < text.len() {
			let ch = text[index];
			if ch == '"' {
				return Read {
					value: Value::String(value),
					complete: true,
					end: index + 1,
				};
			}
			if ch != '\\' {
				value.push(ch);
				index += 1;
				continue;
			}
			let Some(&next) = text.get(index + 1) else {
				return partial(value);
			};
			let simple = match next {
				'"' => Some('"'),
				'\\' => Some('\\'),
				'/' => Some('/'),
				'b' => Some('\u{8}'),
				'f' => Some('\u{c}'),
				'n' => Some('\n'),
				'r' => Some('\r'),
				't' => Some('\t'),
				_ => None,
			};
			if let Some(simple) = simple {
				value.push(simple);
				index += 2;
				continue;
			}
			if next == 'u' {
				let mut end = index + 2;
				while end < index + 6 && end < text.len() && text[end].is_ascii_hexdigit() {
					end += 1;
				}
				if end == index + 6 {
					let hex: String = text[index + 2..end].iter().collect();
					let unit = u16::from_str_radix(&hex, 16).unwrap();
					value.push_str(&String::from_utf16_lossy(&[unit]));
					index = end;
					continue;
				}
				if end == text.len() {
					return partial(value);
				}
				// Not four hex digits: the escape stays literal and the next character is read normally.
				value.extend(&text[index..end]);
				index = end;
				continue;
			}
			// Invalid escape: the backslash stays literal and the next character is read normally.
			value.push('\\');
			index += 1;
		}
		partial(value)
	}

	fn read_object(&self, start: usize) -> Read {
		let text = self.text;
		let mut members = Map::new();
		let partial = |members: Map<String, Value>| Read {
			value: Value::Object(members),
			complete: false,
			end: text.len(),
		};
		let mut index = self.skip_space(start + 1);
		if index >= text.len() {
			return partial(members);
		}
		if text[index] == '}' {
			return Read {
				value: Value::Object(members),
				complete: true,
				end: index + 1,
			};
		}
		loop {
			assert_eq!(text[index], '"', "reference reads valid JSON only");
			let key = self.read_string(index);
			if !key.complete {
				return partial(members);
			}
			index = self.skip_space(key.end);
			if index >= text.len() {
				return partial(members);
			}
			index = self.skip_space(index + 1);
			let Some(member) = self.read_value(index) else {
				return partial(members);
			};
			let Value::String(key) = key.value else { unreachable!() };
			members.insert(key, member.value);
			if !member.complete {
				return partial(members);
			}
			index = self.skip_space(member.end);
			if index >= text.len() {
				return partial(members);
			}
			if text[index] == '}' {
				return Read {
					value: Value::Object(members),
					complete: true,
					end: index + 1,
				};
			}
			index = self.skip_space(index + 1);
			if index >= text.len() {
				return partial(members);
			}
		}
	}

	fn read_array(&self, start: usize) -> Read {
		let text = self.text;
		let mut elements = Vec::new();
		let partial = |elements: Vec<Value>| Read {
			value: Value::Array(elements),
			complete: false,
			end: text.len(),
		};
		let mut index = self.skip_space(start + 1);
		if index >= text.len() {
			return partial(elements);
		}
		if text[index] == ']' {
			return Read {
				value: Value::Array(elements),
				complete: true,
				end: index + 1,
			};
		}
		loop {
			let Some(element) = self.read_value(index) else {
				return partial(elements);
			};
			elements.push(element.value);
			if !element.complete {
				return partial(elements);
			}
			index = self.skip_space(element.end);
			if index >= text.len() {
				return partial(elements);
			}
			if text[index] == ']' {
				return Read {
					value: Value::Array(elements),
					complete: true,
					end: index + 1,
				};
			}
			index = self.skip_space(index + 1);
			if index >= text.len() {
				return partial(elements);
			}
		}
	}

	fn read_scalar(&self, start: usize) -> Option<Read> {
		let text = self.text;
		let mut end = start;
		while end < text.len() && !" \t\n\r,:{}[]\"".contains(text[end]) {
			end += 1;
		}
		let token: String = text[start..end].iter().collect();
		if end < text.len() {
			return Some(Read {
				value: json_parse(&token).unwrap(),
				complete: true,
				end,
			});
		}
		if !self.pending_scalars {
			return None;
		}
		// At the end of the text: a literal prefix, a number, or a number whose exponent has no digits yet.
		for (literal, value) in [("true", json!(true)), ("false", json!(false)), ("null", Value::Null)] {
			if literal.starts_with(&token) {
				return Some(Read {
					value,
					complete: false,
					end,
				});
			}
		}
		let number = regex::Regex::new(r"^-?(?:0|[1-9][0-9]*)(?:\.[0-9]+)?(?:[eE][+-]?[0-9]+)?$").unwrap();
		if number.is_match(&token) {
			return Some(Read {
				value: json_parse(&token).unwrap(),
				complete: false,
				end,
			});
		}
		let mantissa = regex::Regex::new(r"^(-?(?:0|[1-9][0-9]*)(?:\.[0-9]+)?)[eE][+-]?$").unwrap();
		mantissa.captures(&token).map(|captures| Read {
			value: json_parse(&captures[1]).unwrap(),
			complete: false,
			end,
		})
	}
}

#[derive(Clone, Copy)]
enum Chunk {
	Size(usize),
	Random(u32),
}

const CHUNK_SIZES: [Option<usize>; 5] = [Some(1), Some(2), Some(3), Some(7), None];

struct Step {
	read: String,
	snapshot: Option<Rc<Value>>,
	copy: Option<Value>,
}

/// Feed `text` in pieces of characters; returns the snapshot after each piece, a copy taken right then, and the text
/// read by then.
fn stream(text: &str, chunk: Chunk) -> Vec<Step> {
	let chars: Vec<char> = text.chars().collect();
	let mut rand = match chunk {
		Chunk::Random(seed) => Some(Random::new(seed)),
		Chunk::Size(_) => None,
	};
	let mut parser = StreamingJsonParser::new();
	let mut steps = Vec::new();
	let mut index = 0;
	while index < chars.len() {
		let size = match (chunk, &mut rand) {
			(Chunk::Size(size), _) => size,
			(_, Some(rand)) => 1 + rand.index(12),
			_ => unreachable!(),
		};
		let end = (index + size).min(chars.len());
		parser.append(&chars[index..end].iter().collect::<String>());
		index = end;
		let snapshot = parser.snapshot();
		let copy = snapshot.as_deref().cloned();
		steps.push(Step {
			read: chars[..index].iter().collect(),
			snapshot,
			copy,
		});
	}
	steps
}

fn chunk_for(text: &str, size: Option<usize>) -> Chunk {
	size.map_or_else(|| Chunk::Random(read32(text)), Chunk::Size)
}

fn final_snapshot(text: &str, size: usize) -> Option<Value> {
	stream(text, Chunk::Size(size.max(1)))
		.pop()
		.and_then(|step| step.snapshot.as_deref().cloned())
}

/// A stable seed from the text, so random chunk sizes differ per document.
fn read32(text: &str) -> u32 {
	let mut hash: i32 = 7;
	for unit in text.encode_utf16() {
		hash = hash.wrapping_mul(31).wrapping_add(i32::from(unit));
	}
	hash.unsigned_abs() + 1
}

fn char_len(text: &str) -> usize {
	text.chars().count()
}

mod differential_fuzzing {
	use super::*;

	#[test]
	fn matches_the_reference_after_every_piece_at_every_chunk_size_with_insignificant_whitespace() {
		for text in random_documents(1, 250, true) {
			for size in CHUNK_SIZES {
				for step in stream(&text, chunk_for(&text, size)) {
					assert_eq!(
						step.snapshot.as_deref(),
						reference_snapshot(&step.read, true).as_ref(),
						"{:?} (chunk size {size:?})",
						step.read
					);
				}
			}
		}
	}

	#[test]
	fn matches_todays_parse_streaming_json_at_every_prefix_of_compact_documents() {
		for text in random_documents(2, 400, false) {
			for step in stream(&text, Chunk::Size(1)) {
				// Documented differences: parse_streaming_json trims trailing whitespace, and shows an unfinished root
				// `null` as {}.
				if step.read.ends_with(char::is_whitespace) {
					continue;
				}
				let expected = parse_streaming_json(Some(&step.read));
				if step.snapshot.as_deref() == Some(&Value::Null) && "null".starts_with(step.read.as_str()) {
					continue;
				}
				let snapshot = step.snapshot.as_deref().cloned().unwrap_or_else(|| json!({}));
				assert_eq!(snapshot, expected, "{:?}", step.read);
			}
		}
	}

	#[test]
	fn reads_complete_documents_exactly_like_json_parse() {
		for text in random_documents(3, 300, true)
			.into_iter()
			.chain(random_documents(4, 300, false))
		{
			let expected = json_parse(&text).unwrap();
			for size in CHUNK_SIZES {
				assert_eq!(
					final_snapshot(&text, size.unwrap_or(5)),
					Some(expected.clone()),
					"{text}"
				);
			}
		}
	}

	#[test]
	fn never_changes_a_snapshot_once_it_is_returned() {
		for text in random_documents(5, 150, true) {
			for step in stream(&text, Chunk::Random(read32(&text))) {
				assert_eq!(step.snapshot.as_deref(), step.copy.as_ref());
			}
		}
	}

	#[test]
	fn repairs_raw_control_characters_and_invalid_escapes_like_parse_streaming_json_and_shows_them_while_streaming() {
		let mut rand = Random::new(6);
		let malformed = [
			"\n", "\t", "\u{0}", "\\d", "\\x41", "\\u12", "\\uZZZZ", "\\u00e9", "\\\\", "\\q", "a", " ", "é",
		];
		for round in 0..300 {
			let content: String = (0..6).map(|_| rand.pick(&malformed)).collect();
			let text = format!("{{\"path\":\"p\",\"content\":\"{content}\",\"n\":{round}}}");
			// Complete text: the same repaired value as today's parser.
			assert_eq!(
				final_snapshot(&text, 1),
				Some(parse_streaming_json(Some(&text))),
				"{text:?}"
			);
			for step in stream(&text, Chunk::Size(1)) {
				assert_eq!(
					step.snapshot.as_deref(),
					reference_snapshot(&step.read, true).as_ref(),
					"{:?}",
					step.read
				);
			}
		}
	}
}

mod strings {
	use super::*;

	fn split(text: &str, at: usize) -> (String, String) {
		let chars: Vec<char> = text.chars().collect();
		(chars[..at].iter().collect(), chars[at..].iter().collect())
	}

	#[test]
	fn holds_an_escape_back_until_it_is_complete_at_every_split_position() {
		for (text, value) in [
			(r#"{"a":"x\u00e9y"}"#, "xéy"),
			(r#"{"a":"x\ny"}"#, "x\ny"),
			(r#"{"a":"x\\y"}"#, "x\\y"),
			(r#"{"a":"x\"y"}"#, "x\"y"),
			(r#"{"a":"x\/y"}"#, "x/y"),
		] {
			for at in 0..=char_len(text) {
				let (first, second) = split(text, at);
				let mut parser = StreamingJsonParser::new();
				parser.append(&first);
				assert_eq!(
					parser.snapshot().as_deref(),
					reference_snapshot(&first, true).as_ref(),
					"{first}"
				);
				parser.append(&second);
				assert_eq!(parser.snapshot().as_deref(), Some(&json!({ "a": value })));
			}
		}
	}

	#[test]
	fn joins_surrogate_pairs_split_across_pieces_escaped_or_raw() {
		for text in [r#"{"a":"\ud83d\ude00!"}"#, "{\"a\":\"😀!\"}"] {
			for at in 0..=char_len(text) {
				let (first, second) = split(text, at);
				let mut parser = StreamingJsonParser::new();
				parser.append(&first);
				parser.snapshot();
				parser.append(&second);
				assert_eq!(parser.snapshot().as_deref(), Some(&json!({ "a": "😀!" })));
			}
		}
		// A high surrogate whose low half has not arrived is held back: a Rust string cannot hold it alone.
		let mut parser = StreamingJsonParser::new();
		parser.append(r#"{"a":"\ud83d"#);
		assert_eq!(parser.snapshot().as_deref(), Some(&json!({ "a": "" })));
		parser.append(r#"x"}"#);
		assert_eq!(parser.snapshot().as_deref(), Some(&json!({ "a": "\u{fffd}x" })));
	}

	#[test]
	fn keeps_trailing_whitespace_of_an_unterminated_string() {
		let mut parser = StreamingJsonParser::new();
		parser.append("{\"code\":\"x\t\t  ");
		assert_eq!(parser.snapshot().as_deref(), Some(&json!({ "code": "x\t\t  " })));
	}

	#[test]
	fn keeps_raw_control_characters_and_invalid_escapes_visible_while_streaming() {
		let mut parser = StreamingJsonParser::new();
		parser.append("{\"a\":\"x\ny \\d \\u12\"");
		assert_eq!(parser.snapshot().as_deref(), Some(&json!({ "a": "x\ny \\d \\u12" })));
	}

	// #10868
	#[test]
	fn keeps_invalid_u_escapes_literal_without_dropping_fields() {
		assert_eq!(
			final_snapshot(r#"{"path":"a.tex","content":"\usepackage{x}","mode":"w"}"#, 1),
			Some(json!({ "path": "a.tex", "content": "\\usepackage{x}", "mode": "w" }))
		);
		assert_eq!(
			final_snapshot(r#"{"text":"caf\u00e9 \url"}"#, 3),
			Some(json!({ "text": "café \\url" }))
		);
		let mut parser = StreamingJsonParser::new();
		parser.append(r#"{"text":"ab\u00"#);
		assert_eq!(parser.snapshot().as_deref(), Some(&json!({ "text": "ab" })));
	}

	#[test]
	fn makes_proto_an_own_member_as_json_parse_does() {
		let text = r#"{"__proto__":{"polluted":true},"b":{"__proto__":"x"}}"#;
		for at in 0..=char_len(text) {
			let (first, second) = split(text, at);
			let mut parser = StreamingJsonParser::new();
			parser.append(&first);
			parser.snapshot();
			parser.append(&second);
			let value = parser.snapshot().unwrap();
			assert_eq!(*value, json_parse(text).unwrap());
			assert!(value.as_object().unwrap().contains_key("__proto__"));
		}
	}

	#[test]
	fn keeps_the_last_value_of_a_duplicate_key_in_the_first_keys_position_as_json_parse_does() {
		let text = r#"{"a":1,"b":2,"a":"three"}"#;
		let snapshot = final_snapshot(text, 1).unwrap();
		let expected = json_parse(text).unwrap();
		let keys = |value: &Value| value.as_object().unwrap().keys().cloned().collect::<Vec<_>>();
		assert_eq!(keys(&snapshot), keys(&expected));
		assert_eq!(snapshot, expected);
	}

	#[test]
	fn puts_array_index_keys_first_as_json_parse_does() {
		let text = r#"{"b":1,"1":2,"0":3,"4294967295":4}"#;
		let snapshot = final_snapshot(text, 1).unwrap();
		assert_eq!(json_stringify(&snapshot), r#"{"0":3,"1":2,"b":1,"4294967295":4}"#);
		assert_eq!(json_stringify(&snapshot), json_stringify(&json_parse(text).unwrap()));
	}
}

/// Expected values of the inputs, from other partial-JSON parsers' public test suites (see each table's source).
type Vector = (&'static str, Option<Value>);

fn partial_json_vectors() -> Vec<Vector> {
	// partial-json (MIT, https://github.com/promplate/partial-json-parser-js/blob/main/tests/examples.test.js), with
	// every partial type allowed, as parse_streaming_json uses it.
	vec![
		("\"", Some(json!(""))),
		("[\"", Some(json!([""]))),
		("{\"\": \"", Some(json!({ "": "" }))),
		("n", Some(Value::Null)),
		("t", Some(json!(true))),
		("f", Some(json!(false))),
		("0", Some(json!(0))),
		("-1.25e+4", Some(json!(-12500))),
		("-1.25e+", Some(json!(-1.25))),
		("-1.25e", Some(json!(-1.25))),
		// Differences, documented in streaming_json.rs: an invalid escape stays visible (partial-json cuts the string
		// at the backslash), and `Infinity`/`NaN` words are not JSON and are dropped.
		("\" \\x12", Some(json!(" \\x12"))),
		("I", None),
		("-I", None),
		("N", None),
	]
}

fn fix_json_vectors() -> Vec<Vector> {
	// Vercel AI SDK fixJson (Apache-2.0, https://github.com/vercel/ai/blob/main/packages/ai/src/util/fix-json.test.ts):
	// the expected values are JSON.parse() of its repaired texts.
	vec![
		("", None),
		("nul", Some(Value::Null)),
		("t", Some(json!(true))),
		("fals", Some(json!(false))),
		("12.2", Some(json!(12.2))),
		("-12", Some(json!(-12))),
		("-", None),
		("2.5e", Some(json!(2.5))),
		("2.5e-", Some(json!(2.5))),
		("2.5e3", Some(json!(2500))),
		("-2.5e3", Some(json!(-2500))),
		("2.5E", Some(json!(2.5))),
		("2.5E-", Some(json!(2.5))),
		("2.5E3", Some(json!(2500))),
		("-2.5E3", Some(json!(-2500))),
		("12.34e", Some(json!(12.34))),
		("5e", Some(json!(5))),
		("\"abc", Some(json!("abc"))),
		(
			"\"value with \\\"quoted\\\" text and \\\\ escape",
			Some(json!("value with \"quoted\" text and \\ escape")),
		),
		("\"value with \\", Some(json!("value with "))),
		("\"\\u", Some(json!(""))),
		("\"\\u12", Some(json!(""))),
		("\"text \\u00", Some(json!("text "))),
		("{\"a\":\"\\u12", Some(json!({ "a": "" }))),
		("\"value with unicode \u{3c}\"", Some(json!("value with unicode <"))),
		("[", Some(json!([]))),
		("[[1], [2", Some(json!([[1], [2]]))),
		("[[\"1\"], [\"2", Some(json!([["1"], ["2"]]))),
		("[[false], [nu", Some(json!([[false], [null]]))),
		("[[[]], [[]", Some(json!([[[]], [[]]]))),
		("[[{}], [{", Some(json!([[{}], [{}]]))),
		("[1, ", Some(json!([1]))),
		("[[], 123", Some(json!([[], 123]))),
		("{\"key\":", Some(json!({}))),
		(
			"{\"a\": {\"b\": 1}, \"c\": {\"d\": 2",
			Some(json!({ "a": { "b": 1 }, "c": { "d": 2 } })),
		),
		(
			"{\"a\": {\"b\": \"1\"}, \"c\": {\"d\": 2",
			Some(json!({ "a": { "b": "1" }, "c": { "d": 2 } })),
		),
		(
			"{\"a\": {\"b\": false}, \"c\": {\"d\": 2",
			Some(json!({ "a": { "b": false }, "c": { "d": 2 } })),
		),
		(
			"{\"a\": {\"b\": []}, \"c\": {\"d\": 2",
			Some(json!({ "a": { "b": [] }, "c": { "d": 2 } })),
		),
		(
			"{\"a\": {\"b\": {}}, \"c\": {\"d\": 2",
			Some(json!({ "a": { "b": {} }, "c": { "d": 2 } })),
		),
		("{\"ke", Some(json!({}))),
		("{\"k1\": 1, \"k2", Some(json!({ "k1": 1 }))),
		("{\"k1\": 1, \"k2\":", Some(json!({ "k1": 1 }))),
		("{\"key\": \"value\"  ", Some(json!({ "key": "value" }))),
		("{\"a\": {\"b\": {}", Some(json!({ "a": { "b": {} } }))),
		("[1, [2, 3, [", Some(json!([1, [2, 3, []]]))),
		("[false, [true, [", Some(json!([false, [true, []]]))),
		("{\"key\": {\"subKey\":", Some(json!({ "key": {} }))),
		(
			"{\"key\": 123, \"key2\": {\"subKey\":",
			Some(json!({ "key": 123, "key2": {} })),
		),
		(
			"{\"key\": null, \"key2\": {\"subKey\":",
			Some(json!({ "key": null, "key2": {} })),
		),
		("{\"key\": [1, 2, {", Some(json!({ "key": [1, 2, {}] }))),
		("[1, 2, {\"key\": \"value\",", Some(json!([1, 2, { "key": "value" }]))),
		(
			"{\"a\": {\"b\": [\"c\", {\"d\": \"e\",",
			Some(json!({ "a": { "b": ["c", { "d": "e" }] } })),
		),
		(
			"{\"a\": {\"b\": {\"c\": {\"d\":",
			Some(json!({ "a": { "b": { "c": {} } } })),
		),
		("{\"a\": 1, \"b\": [", Some(json!({ "a": 1, "b": [] }))),
		("{\"a\": 1, \"b\": {", Some(json!({ "a": 1, "b": {} }))),
		("{\"a\": 1, \"b\": \"", Some(json!({ "a": 1, "b": "" }))),
		(
			"{\n  \"a\": [\n    {\n      \"a1\": \"v1\",\n      \"a2\": \"v2\",\n      \"a3\": \"v3\"\n    }\n  ],\n  \"b\": [\n    {\n      \"b1\": \"n",
			Some(json!({ "a": [{ "a1": "v1", "a2": "v2", "a3": "v3" }], "b": [{ "b1": "n" }] })),
		),
		(
			"{\"type\":\"div\",\"children\":[{\"type\":\"Card\",\"props\":{}",
			Some(json!({ "type": "div", "children": [{ "type": "Card", "props": {} }] })),
		),
		// Difference: fixJson completes `12.` to 12; like parse_streaming_json, a number cut after its dot is omitted.
		("12.", None),
		("12.e", None),
	]
}

mod ported_test_vectors {
	use super::*;

	#[test]
	fn reads_each_vector() {
		for (input, expected) in partial_json_vectors().into_iter().chain(fix_json_vectors()) {
			for size in [char_len(input).max(1), 1, 2] {
				assert_eq!(final_snapshot(input, size), expected, "{input:?} (chunk size {size})");
			}
		}
	}

	#[test]
	fn reads_every_json_test_suite_document_json_parse_accepts_exactly_like_it_and_never_fails_on_the_rest() {
		let path = concat!(
			env!("CARGO_MANIFEST_DIR"),
			"/../../../packages/ai/test/data/json-test-suite.json"
		);
		let suite = json_parse(&std::fs::read_to_string(path).unwrap()).unwrap();
		for (name, base64) in suite["files"].as_object().unwrap() {
			use base64::Engine;
			let bytes = base64::engine::general_purpose::STANDARD
				.decode(base64.as_str().unwrap())
				.unwrap();
			let text = String::from_utf8_lossy(&bytes).into_owned();
			let expected = json_parse(&text).ok();
			for size in [char_len(&text).max(1), 1, 3] {
				let snapshot = final_snapshot(&text, size);
				if expected.is_some() {
					assert_eq!(snapshot, expected, "{name}");
				}
			}
		}
	}
}

/// A copy of a value kept current by applying the parser's changes, as a writer such as Durable keeps one.
#[derive(Default)]
struct Mirror {
	value: Option<Value>,
}

impl Mirror {
	fn apply(&mut self, changes: &[JsonChange]) {
		for change in changes {
			let (path, value) = match change {
				JsonChange::Set { path, value } => (path, Some(value)),
				JsonChange::Append { path, .. } => (path, None),
			};
			let Some((last, parents)) = path.split_last() else {
				match change {
					JsonChange::Set { value, .. } => self.value = Some(value.clone()),
					JsonChange::Append { text, .. } => {
						let Some(Value::String(current)) = &mut self.value else {
							panic!("append to a string")
						};
						current.push_str(text);
					}
				}
				continue;
			};
			let mut parent = self.value.as_mut().unwrap();
			for segment in parents {
				parent = match segment {
					JsonPathSegment::Key(key) => parent.get_mut(key.as_str()).unwrap(),
					JsonPathSegment::Index(index) => parent.get_mut(*index).unwrap(),
				};
			}
			match value {
				Some(value) => match (parent, last) {
					(Value::Object(members), JsonPathSegment::Key(key)) => {
						members.insert(key.clone(), value.clone());
					}
					(Value::Array(elements), JsonPathSegment::Index(index)) => {
						if *index == elements.len() {
							elements.push(value.clone());
						} else {
							elements[*index] = value.clone();
						}
					}
					_ => panic!("path does not match the mirror"),
				},
				None => {
					let JsonChange::Append { text, .. } = change else {
						unreachable!()
					};
					let target = match last {
						JsonPathSegment::Key(key) => parent.get_mut(key.as_str()),
						JsonPathSegment::Index(index) => parent.get_mut(*index),
					};
					let Some(Value::String(current)) = target else {
						panic!("append to a string")
					};
					current.push_str(text);
				}
			}
		}
	}
}

/// Feed `text` in pieces into a parser and a mirror of its changes; checks the mirror after each piece.
fn mirror_stream(text: &str, chunk: Chunk) -> Mirror {
	let chars: Vec<char> = text.chars().collect();
	let mut rand = match chunk {
		Chunk::Random(seed) => Some(Random::new(seed)),
		Chunk::Size(_) => None,
	};
	let mut parser = StreamingJsonParser::new();
	let mut mirror = Mirror::default();
	let mut index = 0;
	while index < chars.len() {
		let size = match (chunk, &mut rand) {
			(Chunk::Size(size), _) => size,
			(_, Some(rand)) => 1 + rand.index(12),
			_ => unreachable!(),
		};
		let end = (index + size).min(chars.len());
		mirror.apply(&parser.append(&chars[index..end].iter().collect::<String>()));
		index = end;
		let read: String = chars[..index].iter().collect();
		// Changes leave out unterminated numbers and literals, which snapshots show.
		assert_eq!(mirror.value, reference_snapshot(&read, false), "{read:?}");
	}
	mirror
}

mod malformed_text {
	use super::*;

	const MALFORMED: [&str; 17] = [
		"{\"a\":1 \"b\":2}",
		"{\"a\" 1, \"b\": 2}",
		"{a:1, 'b': 2}",
		"{\"a\":1,,\"b\":2,}",
		"[1 2, , 3,]",
		"{\"a\":[1,2}, \"b\":3]",
		"]]]}}}{",
		"{\"a\":NaN,\"b\":Infinity,\"c\":-Infinity,\"d\":undefined,\"e\":1}",
		"{\"a\":01,\"b\":1.,\"c\":.5,\"d\":+1,\"e\":0x10,\"f\":1e}",
		"{\"a\":\"x\"} trailing garbage {\"b\":2}",
		"{\"a\": tru, \"b\": nulll, \"c\": falsey}",
		"{\"a\":{\"b\":{\"c\":",
		"{\"a\\\": 1}",
		"\"unterminated \\",
		"// comment\n{\"a\":1}",
		"\u{0}{\u{1}\"a\"\u{2}:\u{3}1}",
		"{\"a\":\"x\"}}}]]]",
	];

	#[test]
	fn never_fails_and_keeps_snapshots_unchanged_at_every_chunk_size() {
		for text in MALFORMED {
			for size in CHUNK_SIZES {
				let steps = stream(text, chunk_for(text, size));
				for step in &steps {
					assert_eq!(step.snapshot.as_deref(), step.copy.as_ref());
				}
				// Every chunk size reads malformed text the same way.
				assert_eq!(
					steps.last().and_then(|step| step.snapshot.as_deref().cloned()),
					final_snapshot(text, char_len(text)),
					"{text}"
				);
			}
		}
	}

	#[test]
	fn tolerates_missing_commas_and_colons_and_drops_values_in_invalid_positions() {
		let whole = |text: &str| final_snapshot(text, char_len(text));
		assert_eq!(whole("{\"a\":1 \"b\":2}"), Some(json!({ "a": 1, "b": 2 })));
		assert_eq!(whole("{\"a\" 1, \"b\": 2}"), Some(json!({ "a": 1, "b": 2 })));
		assert_eq!(whole("[1 2, , 3,]"), Some(json!([1, 2, 3])));
		assert_eq!(whole("{\"a\":NaN,\"b\":Infinity,\"e\":1}"), Some(json!({ "e": 1 })));
		assert_eq!(
			whole("{\"a\":\"x\"} trailing garbage {\"b\":2}"),
			Some(json!({ "a": "x" }))
		);
	}

	#[test]
	fn survives_random_corruption_of_valid_documents() {
		let mut rand = Random::new(7);
		let noise = ['"', '\\', '{', '}', '[', ']', ',', ':', 'x', '1', '-', '\n', 't'];
		for text in random_documents(8, 300, true) {
			let mut characters: Vec<char> = text.chars().collect();
			for _ in 0..3 {
				let at = rand.index(characters.len() + 1);
				if rand.next() < 0.5 {
					if at < characters.len() {
						characters.remove(at);
					}
				} else {
					characters.insert(at, rand.pick(&noise));
				}
			}
			let corrupted: String = characters.iter().collect();
			stream(&corrupted, Chunk::Random(read32(&corrupted)));
			// The changes keep a copy equal to the parser's own value, malformed or not.
			let mut parser = StreamingJsonParser::new();
			let mut mirror = Mirror::default();
			for piece in characters.chunks(4) {
				mirror.apply(&parser.append(&piece.iter().collect::<String>()));
			}
			// A space ends a number or literal at the end, which snapshots show and changes do not report yet.
			mirror.apply(&parser.append(" "));
			assert_eq!(mirror.value.as_ref(), parser.snapshot().as_deref(), "{corrupted}");
		}
	}
}

mod changes {
	use super::*;

	fn set(path: Vec<JsonPathSegment>, value: Value) -> JsonChange {
		JsonChange::Set { path, value }
	}

	fn key(key: &str) -> JsonPathSegment {
		JsonPathSegment::Key(key.into())
	}

	fn index(index: usize) -> JsonPathSegment {
		JsonPathSegment::Index(index)
	}

	#[test]
	fn keep_a_copy_equal_to_the_value_read_so_far_at_every_chunk_size_and_to_json_parse_at_the_end() {
		for text in random_documents(9, 200, true) {
			for size in CHUNK_SIZES {
				let mirror = mirror_stream(&text, chunk_for(&text, size));
				assert_eq!(mirror.value, Some(json_parse(&text).unwrap()), "{text}");
			}
		}
	}

	#[test]
	fn report_a_number_that_ends_and_a_string_that_starts_in_one_piece_in_order() {
		let mut parser = StreamingJsonParser::new();
		assert_eq!(parser.append("{\"n\":1"), vec![set(vec![], json!({}))]);
		assert_eq!(
			parser.append("2,\"s\":\"ab"),
			vec![set(vec![key("n")], json!(12)), set(vec![key("s")], json!("ab"))]
		);
		assert_eq!(
			parser.append("c\",\"t\":tru"),
			vec![JsonChange::Append {
				path: vec![key("s")],
				text: "c".into()
			}]
		);
		assert_eq!(parser.append("e}"), vec![set(vec![key("t")], json!(true))]);
		assert_eq!(parser.append(" {\"ignored\":1}"), vec![]);
	}

	#[test]
	fn report_nested_objects_and_arrays_as_empty_containers_then_their_members_and_items() {
		let mut parser = StreamingJsonParser::new();
		assert_eq!(
			parser.append("{\"a\":[1,{\"b\":[\"x\""),
			vec![
				set(vec![], json!({})),
				set(vec![key("a")], json!([])),
				set(vec![key("a"), index(0)], json!(1)),
				set(vec![key("a"), index(1)], json!({})),
				set(vec![key("a"), index(1), key("b")], json!([])),
				set(vec![key("a"), index(1), key("b"), index(0)], json!("x")),
			]
		);
		assert_eq!(
			parser.append(",\"y"),
			vec![set(vec![key("a"), index(1), key("b"), index(1)], json!("y"))]
		);
		assert_eq!(
			parser.append("z\"]}],\"a\":\"again\"}"),
			vec![
				JsonChange::Append {
					path: vec![key("a"), index(1), key("b"), index(1)],
					text: "z".into()
				},
				set(vec![key("a")], json!("again")),
			]
		);
	}

	#[test]
	fn hold_split_escapes_back_and_report_their_decoded_text_once_complete() {
		let text = r#"{"a":"x\u00e9\n\\\"\ud83d\ude00\q"}"#;
		let expected = json_parse(r#"{"a":"x\u00e9\n\\\"\ud83d\ude00\\q"}"#).unwrap();
		let chars: Vec<char> = text.chars().collect();
		for first in 1..chars.len() {
			for second in first..chars.len() {
				let mut parser = StreamingJsonParser::new();
				let mut mirror = Mirror::default();
				for piece in [&chars[..first], &chars[first..second], &chars[second..]] {
					mirror.apply(&parser.append(&piece.iter().collect::<String>()));
				}
				assert_eq!(mirror.value.as_ref(), Some(&expected));
			}
		}
		let mut parser = StreamingJsonParser::new();
		parser.append("{\"a\":\"x");
		assert_eq!(parser.append("\\u00"), vec![]);
		assert_eq!(
			parser.append("e9"),
			vec![JsonChange::Append {
				path: vec![key("a")],
				text: "é".into()
			}]
		);
	}

	#[test]
	fn report_a_container_nested_deeper_than_32_levels_once_whole_when_it_closes() {
		let depth = 40;
		let text = format!("{}\"deep\"{}", "{\"a\":".repeat(depth), "}".repeat(depth));
		let mut parser = StreamingJsonParser::new();
		let mut mirror = Mirror::default();
		let chars: Vec<char> = text.chars().collect();
		for piece in chars.chunks(3) {
			mirror.apply(&parser.append(&piece.iter().collect::<String>()));
		}
		assert_eq!(mirror.value, Some(json_parse(&text).unwrap()));
		let changes = StreamingJsonParser::new().append(&text);
		// The containers down to path length 32, then the one at length 32 again, whole, as it closes.
		assert_eq!(changes.len(), 34);
		let JsonChange::Set { path, value } = changes.last().unwrap() else {
			panic!("expected a set");
		};
		assert_eq!(path.len(), 32);
		assert_eq!(*value, json_parse(&text[32 * 5..text.len() - 32]).unwrap());
	}

	#[test]
	fn report_nothing_for_values_in_invalid_positions_and_text_after_the_root() {
		let mut parser = StreamingJsonParser::new();
		assert_eq!(
			parser.append("{\"a\":1 2, \"b\" \"x\"}"),
			vec![
				set(vec![], json!({})),
				set(vec![key("a")], json!(1)),
				set(vec![key("b")], json!("x")),
			]
		);
		assert_eq!(parser.append("{\"c\":3}"), vec![]);
	}

	#[test]
	fn serialize_like_the_ts_change_objects() {
		let change = set(vec![key("a"), index(0)], json!(1));
		assert_eq!(json_stringify(&change), r#"{"kind":"set","path":["a",0],"value":1}"#);
		let change = JsonChange::Append {
			path: vec![],
			text: "x".into(),
		};
		assert_eq!(json_stringify(&change), r#"{"kind":"append","path":[],"text":"x"}"#);
	}
}

mod cost {
	use super::*;

	fn write_arguments(size: usize) -> String {
		let content: String = (0..20_000)
			.map(|line| format!("const line{line} = \"value {line}\"; // \\\\ done\n"))
			.collect();
		json_stringify(&json!({ "path": "/src/file.ts", "content": &content[..size] }))
	}

	fn stream_arguments(text: &str, delta: usize) {
		let mut args = ToolCallArgumentsStream::new();
		for piece in text.as_bytes().chunks(delta) {
			args.append(std::str::from_utf8(piece).unwrap());
		}
		args.finish();
	}

	#[test]
	fn streams_large_arguments_in_linear_time() {
		let small = write_arguments(50_000);
		let large = write_arguments(500_000);
		let started = Instant::now();
		stream_arguments(&small, 32);
		let small_time = started.elapsed();
		let started = Instant::now();
		stream_arguments(&large, 32);
		let large_time = started.elapsed();
		// Ten times the text: linear growth is about 10x, quadratic about 100x.
		assert!(
			large_time.as_secs_f64() / small_time.as_secs_f64().max(1e-3) < 40.0,
			"{small_time:?} vs {large_time:?}"
		);
	}

	#[test]
	fn keeps_very_large_open_arrays_linear_by_letting_their_preview_lag() {
		let elements = |count: usize| json_stringify(&json!({ "items": (0..count).collect::<Vec<_>>() }));
		let large = elements(100_000);
		let mut parser = StreamingJsonParser::new();
		let mut shown = 0;
		for piece in large.as_bytes().chunks(16) {
			parser.append(std::str::from_utf8(piece).unwrap());
			let items = parser
				.snapshot()
				.and_then(|snapshot| snapshot.get("items").and_then(Value::as_array).map(Vec::len))
				.unwrap_or(0);
			// The preview only moves forward.
			assert!(items >= shown);
			shown = items;
		}
		assert_eq!(parser.snapshot().as_deref(), Some(&json_parse(&large).unwrap()));
	}

	#[test]
	fn reads_deep_nesting_without_recursion_and_in_linear_time() {
		let depth = 200_000;
		let text = format!("{}1{}", "[".repeat(depth), "]".repeat(depth));
		let started = Instant::now();
		let mut parser = StreamingJsonParser::new();
		for piece in text.as_bytes().chunks(7) {
			for change in parser.append(std::str::from_utf8(piece).unwrap()) {
				if let JsonChange::Set { value, .. } = change {
					drop_deep(value);
				}
			}
			parser.snapshot();
		}
		let snapshot = parser.snapshot().unwrap();
		let mut value = &*snapshot;
		for _ in 0..depth {
			value = &value[0];
		}
		assert_eq!(*value, json!(1));
		assert!(started.elapsed().as_secs() < 30, "{:?}", started.elapsed());
		drop(snapshot);
	}

	#[test]
	fn reads_one_huge_piece_such_as_a_providers_complete_arguments_in_linear_time() {
		let text = write_arguments(500_000);
		let started = Instant::now();
		let mut parser = StreamingJsonParser::new();
		parser.append(&text);
		let snapshot = parser.snapshot().unwrap();
		assert_eq!(snapshot["path"], "/src/file.ts");
		assert!(started.elapsed().as_secs() < 10, "{:?}", started.elapsed());
	}
}

mod tool_call_arguments_stream {
	use super::*;

	fn object(value: Value) -> pi_ai::types::JsonObject {
		value.as_object().unwrap().clone()
	}

	// Rust-only: TS places a string when it starts, so closing a string the last snapshot showed changes nothing.
	#[test]
	fn keeps_the_same_arguments_when_a_shown_string_closes() {
		let mut args = ToolCallArgumentsStream::new();
		let open = args.append("{\"a\":\"x");
		assert!(Rc::ptr_eq(&args.append("\""), &open));
		assert!(!Rc::ptr_eq(&args.append(",\"b\":1"), &open));
	}

	#[test]
	fn shows_partial_arguments_as_an_object_and_finishes_with_one_full_parse() {
		let mut args = ToolCallArgumentsStream::new();
		assert_eq!(*args.append("  "), object(json!({})));
		assert_eq!(
			*args.append("{\"path\":\"a.ts\",\"content\":\"x"),
			object(json!({ "path": "a.ts", "content": "x" }))
		);
		let before = args.append("y");
		assert!(Rc::ptr_eq(&args.append(""), &before));
		assert_eq!(args.text(), "  {\"path\":\"a.ts\",\"content\":\"xy");
		assert_eq!(
			args.finish(),
			parse_tool_call_arguments(Some("  {\"path\":\"a.ts\",\"content\":\"xy"))
		);
		args.append("\"}");
		let finished = args.finish();
		assert_eq!(finished.arguments, object(json!({ "path": "a.ts", "content": "xy" })));
		assert_eq!(finished.arguments_error, None);
	}

	#[test]
	fn shows_non_object_roots_as_no_arguments_yet() {
		let mut args = ToolCallArgumentsStream::new();
		assert_eq!(*args.append("[1,2"), object(json!({})));
		args.append("]");
		assert!(
			args.finish()
				.arguments_error
				.unwrap()
				.starts_with("Tool arguments must be a JSON object")
		);
	}

	#[test]
	fn replaces_the_text_with_the_providers_complete_text_returning_the_missing_piece() {
		let mut args = ToolCallArgumentsStream::new();
		args.append("{\"a\":\"x");
		let replaced = args.replace("{\"a\":\"xyz\"}");
		assert_eq!(replaced.delta.as_deref(), Some("yz\"}"));
		assert_eq!(*replaced.arguments, object(json!({ "a": "xyz" })));
		let replaced = args.replace("{\"b\":2}");
		assert_eq!(replaced.delta, None);
		assert_eq!(*replaced.arguments, object(json!({ "b": 2 })));
		assert_eq!(args.text(), "{\"b\":2}");
	}
}

/// Drops `value` without recursion: a value nested 200,000 levels deep overflows the stack of a recursive drop.
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
