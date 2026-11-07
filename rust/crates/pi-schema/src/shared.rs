//! Port of `packages/schema/src/shared.ts`.
//! PORT-STATUS: ported
//!
//! Regular expressions shared across all compiled schemas while one still uses them: tool schemas repeat a few
//! patterns many times, and a RegExp is large once it has run. The cache holds its values weakly and drops an entry
//! once its RegExp is dropped, so it keeps nothing alive.
//!
//! Rust-only: [`JsRegex`] stands in for the ECMAScript `RegExp`. It parses the ECMAScript pattern syntax (with and
//! without the `u` flag, including the Annex B rules that apply without it), rejects what `new RegExp` rejects, and
//! translates the pattern to `fancy_regex` syntax with JavaScript's meaning of `.`, `\d`, `\w`, `\s` and `\b`.
//! Known gaps: patterns run on Unicode scalar values, so lone surrogates cannot occur and a non-`u` pattern sees an
//! astral character as one character, not two UTF-16 units; a backreference to a group that did not participate
//! fails instead of matching empty; `\p{...}` names are checked by shape and by the Rust Unicode tables, not by
//! ECMAScript's exact list; case-insensitive matching uses Unicode simple case folding in both modes. Error messages
//! follow V8's `Invalid regular expression: /source/flags: reason` form; some reasons are worded differently.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::{Rc, Weak};

use pi_host::{PiError, PiResult};

/// The regular expression of a `pattern` source, shared by source. Patterns are ECMAScript regular expressions with
/// the `u` flag; a source that is invalid with it (some older schemas escape characters `u` rejects) compiles
/// without it.
pub fn shared_regex(source: &str) -> PiResult<Rc<JsRegex>> {
	let hit = REGEXES.with(|regexes| regexes.borrow().get(source).and_then(Weak::upgrade));
	if let Some(hit) = hit {
		return Ok(hit);
	}
	let re = match JsRegex::new(source, "u") {
		Ok(re) => re,
		Err(_) => JsRegex::new(source, "")?,
	};
	let re = Rc::new(re);
	REGEXES.with(|regexes| regexes.borrow_mut().insert(source.to_string(), Rc::downgrade(&re)));
	Ok(re)
}

thread_local! {
	static REGEXES: RefCell<HashMap<String, Weak<JsRegex>>> = RefCell::new(HashMap::new());
}

/// The number of cached regular expressions (for tests).
pub fn shared_regex_count() -> usize {
	REGEXES.with(|regexes| regexes.borrow().len())
}

/// An ECMAScript regular expression (see the file header).
#[derive(Debug)]
pub struct JsRegex {
	source: String,
	regex: fancy_regex::Regex,
}

impl JsRegex {
	/// `new RegExp(source, flags)` for the flags `i`, `m`, `s` and `u` (`g` and `y` are accepted and ignored).
	/// Throws a SyntaxError for an invalid pattern, as `new RegExp` does.
	pub fn new(source: &str, flags: &str) -> PiResult<JsRegex> {
		let mut unicode = false;
		let mut prefix = String::new();
		let mut dot_all = false;
		let mut seen = String::new();
		for flag in flags.chars() {
			if seen.contains(flag) || !"dgimsuy".contains(flag) {
				return Err(PiError::named(
					"SyntaxError",
					format!("Invalid flags supplied to RegExp constructor '{flags}'"),
				));
			}
			seen.push(flag);
			match flag {
				'u' => unicode = true,
				'i' => prefix.push('i'),
				'm' => prefix.push('m'),
				's' => dot_all = true,
				_ => {}
			}
		}
		let fail = |reason: &str| {
			PiError::named(
				"SyntaxError",
				format!("Invalid regular expression: /{source}/{flags}: {reason}"),
			)
		};
		let translated = Translator::run(source, unicode, dot_all).map_err(|reason| fail(&reason))?;
		let pattern = if prefix.is_empty() {
			translated
		} else {
			format!("(?{prefix}){translated}")
		};
		let regex = fancy_regex::RegexBuilder::new(&pattern)
			.backtrack_limit(100_000_000)
			.build()
			.map_err(|error| fail(&error.to_string()))?;
		Ok(JsRegex {
			source: source.to_string(),
			regex,
		})
	}

	/// `re.source`.
	pub fn source(&self) -> &str {
		&self.source
	}

	/// `re.test(value)` (for a regular expression without the `g` flag).
	pub fn test(&self, value: &str) -> bool {
		self.regex.is_match(value).unwrap_or(false)
	}

	/// `re.exec(value)`: the match and its groups (`None` for a group that did not participate), or `None`.
	pub fn exec(&self, value: &str) -> Option<Vec<Option<String>>> {
		let captures = self.regex.captures(value).ok().flatten()?;
		Some(
			captures
				.iter()
				.map(|group| group.map(|m| m.as_str().to_string()))
				.collect(),
		)
	}

	/// The byte range of the first match in `value`.
	pub fn find(&self, value: &str) -> Option<(usize, usize)> {
		let found = self.regex.find(value).ok().flatten()?;
		Some((found.start(), found.end()))
	}
}

impl Drop for JsRegex {
	// The FinalizationRegistry of the TypeScript: once the last user drops a regular expression, its entry goes.
	fn drop(&mut self) {
		let _ = REGEXES.try_with(|regexes| {
			if let Ok(mut regexes) = regexes.try_borrow_mut()
				&& regexes.get(&self.source).is_some_and(|entry| entry.upgrade().is_none())
			{
				regexes.remove(&self.source);
			}
		});
	}
}

// `\s`: WhiteSpace and LineTerminator.
const SPACE_RANGES: &[(u32, u32)] = &[
	(0x09, 0x0d),
	(0x20, 0x20),
	(0xa0, 0xa0),
	(0x1680, 0x1680),
	(0x2000, 0x200a),
	(0x2028, 0x2029),
	(0x202f, 0x202f),
	(0x205f, 0x205f),
	(0x3000, 0x3000),
	(0xfeff, 0xfeff),
];
const DIGIT_RANGES: &[(u32, u32)] = &[(0x30, 0x39)];
const WORD_RANGES: &[(u32, u32)] = &[(0x30, 0x39), (0x41, 0x5a), (0x5f, 0x5f), (0x61, 0x7a)];

/// A class escape: `\d`, `\s`, `\w` (negated for the capitals), or `\p{...}` / `\P{...}`.
#[derive(Clone)]
enum ClassEscape {
	Ranges(&'static [(u32, u32)], bool),
	Property(String, bool),
}

impl ClassEscape {
	// The class items of this escape, for use inside a bracketed class.
	fn items(&self) -> String {
		match self {
			ClassEscape::Ranges(ranges, false) => ranges_text(ranges),
			ClassEscape::Ranges(ranges, true) => format!("[^{}]", ranges_text(ranges)),
			ClassEscape::Property(name, false) => format!("\\p{{{name}}}"),
			ClassEscape::Property(name, true) => format!("\\P{{{name}}}"),
		}
	}
}

fn ranges_text(ranges: &[(u32, u32)]) -> String {
	let mut out = String::new();
	for &(lo, hi) in ranges {
		push_range(&mut out, lo, hi);
	}
	out
}

// A code point range inside a bracketed class, without the surrogates a Rust string cannot contain.
fn push_range(out: &mut String, lo: u32, hi: u32) {
	let mut parts = Vec::new();
	if lo < 0xd800 {
		parts.push((lo, hi.min(0xd7ff)));
	}
	if hi > 0xdfff {
		parts.push((lo.max(0xe000), hi));
	}
	for (lo, hi) in parts {
		if lo == hi {
			out.push_str(&format!("\\x{{{lo:X}}}"));
		} else {
			out.push_str(&format!("\\x{{{lo:X}}}-\\x{{{hi:X}}}"));
		}
	}
}

/// One atom of a bracketed class.
enum ClassAtom {
	Char(u32),
	Escape(ClassEscape),
}

/// The parser of an ECMAScript pattern that writes its fancy_regex translation.
struct Translator {
	chars: Vec<char>,
	pos: usize,
	unicode: bool,
	/// The number of capturing groups and their names, from a scan before parsing.
	groups: usize,
	names: Vec<String>,
	dot_all: Vec<bool>,
	out: String,
}

/// What a term was: an assertion that may not be quantified, a lookahead (quantifiable without `u`), or an atom.
enum Term {
	Assertion,
	Lookahead,
	Atom,
}

impl Translator {
	fn run(source: &str, unicode: bool, dot_all: bool) -> Result<String, String> {
		let chars: Vec<char> = source.chars().collect();
		let (groups, names) = scan_groups(&chars);
		let mut t = Translator {
			chars,
			pos: 0,
			unicode,
			groups,
			names,
			dot_all: vec![dot_all],
			out: String::new(),
		};
		t.disjunction()?;
		if t.pos < t.chars.len() {
			return Err(if t.chars[t.pos] == ')' {
				"Unmatched ')'".into()
			} else {
				"Unexpected character".into()
			});
		}
		for name in &t.names {
			if t.names.iter().filter(|n| *n == name).count() > 1 {
				return Err("Duplicate capture group name".into());
			}
		}
		Ok(t.out)
	}

	fn peek(&self) -> Option<char> {
		self.chars.get(self.pos).copied()
	}

	fn peek_at(&self, offset: usize) -> Option<char> {
		self.chars.get(self.pos + offset).copied()
	}

	fn eat(&mut self, c: char) -> bool {
		if self.peek() == Some(c) {
			self.pos += 1;
			true
		} else {
			false
		}
	}

	fn disjunction(&mut self) -> Result<(), String> {
		self.alternative()?;
		while self.eat('|') {
			self.out.push('|');
			self.alternative()?;
		}
		Ok(())
	}

	fn alternative(&mut self) -> Result<(), String> {
		while let Some(c) = self.peek() {
			if c == '|' || c == ')' {
				break;
			}
			let start = self.out.len();
			let term = self.term()?;
			self.quantifier(term, start)?;
		}
		Ok(())
	}

	fn term(&mut self) -> Result<Term, String> {
		let c = self.peek().expect("term() runs before the end of the pattern");
		match c {
			'^' | '$' => {
				self.pos += 1;
				self.out.push(c);
				Ok(Term::Assertion)
			}
			'(' => self.group(),
			'.' => {
				self.pos += 1;
				if *self.dot_all.last().expect("the dotAll stack is never empty") {
					self.out.push_str("(?s:.)");
				} else {
					self.out.push_str("[^\\n\\r\\x{2028}\\x{2029}]");
				}
				Ok(Term::Atom)
			}
			'[' => {
				self.class()?;
				Ok(Term::Atom)
			}
			'\\' => self.atom_escape(),
			'*' | '+' | '?' => Err("Nothing to repeat".into()),
			'{' => {
				if self.unicode {
					return Err(if self.braced_quantifier().is_some() {
						"Nothing to repeat".into()
					} else {
						"Lone quantifier brackets".into()
					});
				}
				if self.braced_quantifier().is_some() {
					return Err("Nothing to repeat".into());
				}
				self.pos += 1;
				self.literal(u32::from('{'));
				Ok(Term::Atom)
			}
			'}' | ']' => {
				if self.unicode {
					return Err("Lone quantifier brackets".into());
				}
				self.pos += 1;
				self.literal(u32::from(c));
				Ok(Term::Atom)
			}
			_ => {
				self.pos += 1;
				self.literal(u32::from(c));
				Ok(Term::Atom)
			}
		}
	}

	// `{n}`, `{n,}` or `{n,m}` at the current position: its bounds and length, without consuming it.
	fn braced_quantifier(&self) -> Option<(u64, Option<u64>, usize)> {
		let mut i = self.pos;
		if self.chars.get(i) != Some(&'{') {
			return None;
		}
		i += 1;
		let digits = |i: &mut usize| -> Option<u64> {
			let start = *i;
			while self.chars.get(*i).is_some_and(char::is_ascii_digit) {
				*i += 1;
			}
			if *i == start {
				return None;
			}
			let text: String = self.chars[start..*i].iter().collect();
			Some(text.parse::<u64>().unwrap_or(u64::MAX))
		};
		let min = digits(&mut i)?;
		let max = if self.chars.get(i) == Some(&',') {
			i += 1;
			if self.chars.get(i) == Some(&'}') {
				None
			} else {
				Some(digits(&mut i)?)
			}
		} else {
			Some(min)
		};
		if self.chars.get(i) != Some(&'}') {
			return None;
		}
		Some((min, max, i + 1 - self.pos))
	}

	fn quantifier(&mut self, term: Term, start: usize) -> Result<(), String> {
		let (min, text) = match self.peek() {
			Some('*') => (0, "*".to_string()),
			Some('+') => (1, "+".to_string()),
			Some('?') => (0, "?".to_string()),
			Some('{') => match self.braced_quantifier() {
				Some((min, max, len)) => {
					if max.is_some_and(|max| max < min) {
						return Err("numbers out of order in {} quantifier".into());
					}
					let text: String = self.chars[self.pos..self.pos + len].iter().collect();
					self.pos += len - 1;
					(min, text)
				}
				None => return Ok(()),
			},
			_ => return Ok(()),
		};
		self.pos += 1;
		let lazy = self.eat('?');
		match term {
			Term::Assertion => return Err("Nothing to repeat".into()),
			Term::Lookahead if self.unicode => return Err("Invalid quantifier".into()),
			// A repeated assertion is the assertion once, or nothing when it may repeat zero times.
			Term::Lookahead => {
				if min == 0 {
					self.out.truncate(start);
				}
				return Ok(());
			}
			Term::Atom => {}
		}
		let atom = self.out.split_off(start);
		self.out.push_str("(?:");
		self.out.push_str(&atom);
		self.out.push(')');
		self.out.push_str(&text);
		if lazy {
			self.out.push('?');
		}
		Ok(())
	}

	fn group(&mut self) -> Result<Term, String> {
		self.pos += 1;
		if !self.eat('?') {
			self.out.push('(');
			self.disjunction()?;
			self.close_group()?;
			return Ok(Term::Atom);
		}
		match self.peek() {
			Some(':') => {
				self.pos += 1;
				self.out.push_str("(?:");
				self.disjunction()?;
				self.close_group()?;
				Ok(Term::Atom)
			}
			Some('=') | Some('!') => {
				let c = self.peek().expect("matched above");
				self.pos += 1;
				self.out.push_str(if c == '=' { "(?=" } else { "(?!" });
				self.disjunction()?;
				self.close_group()?;
				Ok(Term::Lookahead)
			}
			Some('<') if matches!(self.peek_at(1), Some('=') | Some('!')) => {
				let c = self.peek_at(1).expect("matched above");
				self.pos += 2;
				self.out.push_str(if c == '=' { "(?<=" } else { "(?<!" });
				self.disjunction()?;
				self.close_group()?;
				Ok(Term::Assertion)
			}
			Some('<') => {
				self.pos += 1;
				let name = self.group_name()?;
				self.out.push_str(&format!("(?P<{}>", rust_group_name(&name)));
				self.disjunction()?;
				self.close_group()?;
				Ok(Term::Atom)
			}
			_ => self.modifiers(),
		}
	}

	// `(?ims-ims:...)`: inline modifiers.
	fn modifiers(&mut self) -> Result<Term, String> {
		let mut add = String::new();
		let mut remove = String::new();
		let mut removing = false;
		loop {
			match self.peek() {
				Some(c @ ('i' | 'm' | 's')) => {
					if add.contains(c) || remove.contains(c) {
						return Err("Repeated flag in modifiers".into());
					}
					if removing {
						remove.push(c)
					} else {
						add.push(c)
					}
					self.pos += 1;
				}
				Some('-') if !removing => {
					removing = true;
					self.pos += 1;
				}
				Some(':') => {
					self.pos += 1;
					break;
				}
				_ => return Err("Invalid group".into()),
			}
		}
		if removing && remove.is_empty() && add.is_empty() {
			return Err("Invalid group".into());
		}
		if !removing && add.is_empty() {
			return Err("Invalid group".into());
		}
		let mut flags = String::new();
		for c in add.chars().filter(|c| *c != 's') {
			flags.push(c);
		}
		let negative: String = remove.chars().filter(|c| *c != 's').collect();
		if !negative.is_empty() {
			flags.push('-');
			flags.push_str(&negative);
		}
		let dot_all = if add.contains('s') {
			true
		} else if remove.contains('s') {
			false
		} else {
			*self.dot_all.last().expect("the dotAll stack is never empty")
		};
		self.dot_all.push(dot_all);
		if flags.is_empty() {
			self.out.push_str("(?:");
		} else {
			self.out.push_str(&format!("(?{flags}:"));
		}
		self.disjunction()?;
		self.close_group()?;
		self.dot_all.pop();
		Ok(Term::Atom)
	}

	fn close_group(&mut self) -> Result<(), String> {
		if !self.eat(')') {
			return Err("Unterminated group".into());
		}
		self.out.push(')');
		Ok(())
	}

	// A group name after `(?<` or `\k<`, through the closing `>`.
	fn group_name(&mut self) -> Result<String, String> {
		let mut name = String::new();
		loop {
			match self.peek() {
				Some('>') => {
					self.pos += 1;
					break;
				}
				Some(c) if is_name_char(c, name.is_empty()) => {
					name.push(c);
					self.pos += 1;
				}
				_ => return Err("Invalid capture group name".into()),
			}
		}
		if name.is_empty() {
			return Err("Invalid capture group name".into());
		}
		Ok(name)
	}

	fn atom_escape(&mut self) -> Result<Term, String> {
		self.pos += 1;
		let Some(c) = self.peek() else {
			return Err("\\ at end of pattern".into());
		};
		match c {
			'b' => {
				self.pos += 1;
				self.out.push_str(WORD_BOUNDARY);
				return Ok(Term::Assertion);
			}
			'B' => {
				self.pos += 1;
				self.out.push_str(NOT_WORD_BOUNDARY);
				return Ok(Term::Assertion);
			}
			'1'..='9' => {
				let start = self.pos;
				let mut n: usize = 0;
				while let Some(d) = self.peek().and_then(|d| d.to_digit(10)) {
					n = n.saturating_mul(10).saturating_add(d as usize);
					self.pos += 1;
				}
				if n <= self.groups {
					self.out.push_str(&format!("\\{n}"));
					return Ok(Term::Atom);
				}
				if self.unicode {
					return Err("Invalid escape".into());
				}
				self.pos = start;
				let value = self.legacy_octal();
				self.literal(value);
				return Ok(Term::Atom);
			}
			'k' if self.unicode || !self.names.is_empty() => {
				self.pos += 1;
				if !self.eat('<') {
					return Err("Invalid named reference".into());
				}
				let name = self.group_name().map_err(|_| "Invalid named reference".to_string())?;
				if !self.names.contains(&name) {
					return Err("Invalid named capture referenced".into());
				}
				self.out.push_str(&format!("\\k<{}>", rust_group_name(&name)));
				return Ok(Term::Atom);
			}
			_ => {}
		}
		if let Some(escape) = self.class_escape()? {
			self.out.push('[');
			self.out.push_str(&escape.items());
			self.out.push(']');
			return Ok(Term::Atom);
		}
		let value = self.character_escape(false)?;
		self.literal(value);
		Ok(Term::Atom)
	}

	// A legacy octal escape (Annex B): up to three octal digits, at most \377; an 8 or 9 is itself.
	fn legacy_octal(&mut self) -> u32 {
		let c = self.peek().expect("legacy_octal() runs on a digit");
		if c == '8' || c == '9' {
			self.pos += 1;
			return u32::from(c);
		}
		let mut value = 0;
		let mut count = 0;
		while count < 3 {
			let Some(d) = self.peek().and_then(|d| d.to_digit(8)) else {
				break;
			};
			if value * 8 + d > 0o377 {
				break;
			}
			value = value * 8 + d;
			self.pos += 1;
			count += 1;
		}
		value
	}

	// `\d`, `\D`, `\s`, `\S`, `\w`, `\W`, and with `u` `\p{...}` and `\P{...}` (after the backslash).
	fn class_escape(&mut self) -> Result<Option<ClassEscape>, String> {
		let c = self.peek().expect("class_escape() runs after a backslash");
		let escape = match c {
			'd' => ClassEscape::Ranges(DIGIT_RANGES, false),
			'D' => ClassEscape::Ranges(DIGIT_RANGES, true),
			's' => ClassEscape::Ranges(SPACE_RANGES, false),
			'S' => ClassEscape::Ranges(SPACE_RANGES, true),
			'w' => ClassEscape::Ranges(WORD_RANGES, false),
			'W' => ClassEscape::Ranges(WORD_RANGES, true),
			'p' | 'P' if self.unicode => {
				self.pos += 1;
				if !self.eat('{') {
					return Err("Invalid property name".into());
				}
				let mut name = String::new();
				loop {
					match self.peek() {
						Some('}') => {
							self.pos += 1;
							break;
						}
						Some(ch) if ch.is_ascii_alphanumeric() || ch == '_' || ch == '=' => {
							name.push(ch);
							self.pos += 1;
						}
						_ => return Err("Invalid property name".into()),
					}
				}
				if !valid_property(&name) {
					return Err("Invalid property name".into());
				}
				let name = match name.rsplit('=').next().and_then(general_category_alias) {
					Some(category) => category.to_string(),
					None => name,
				};
				return Ok(Some(ClassEscape::Property(name, c == 'P')));
			}
			_ => return Ok(None),
		};
		self.pos += 1;
		Ok(Some(escape))
	}

	// A CharacterEscape or IdentityEscape after the backslash: its code point. `in_class` allows the class-only forms.
	fn character_escape(&mut self, in_class: bool) -> Result<u32, String> {
		let c = self.peek().expect("character_escape() runs after a backslash");
		self.pos += 1;
		match c {
			'f' => Ok(0x0c),
			'n' => Ok(0x0a),
			'r' => Ok(0x0d),
			't' => Ok(0x09),
			'v' => Ok(0x0b),
			'c' => {
				if let Some(letter) = self.peek().filter(char::is_ascii_alphabetic) {
					self.pos += 1;
					return Ok(u32::from(letter) % 32);
				}
				if in_class
					&& !self.unicode
					&& let Some(other) = self.peek().filter(|ch| ch.is_ascii_digit() || *ch == '_')
				{
					self.pos += 1;
					return Ok(u32::from(other) % 32);
				}
				if self.unicode {
					return Err("Invalid unicode escape".into());
				}
				// Annex B: "\c" not followed by a control letter is a backslash and a "c".
				self.pos -= 1;
				Ok(u32::from('\\'))
			}
			'0' if !self.peek().is_some_and(|d| d.is_ascii_digit()) => Ok(0),
			'0'..='9' => {
				if self.unicode {
					return Err(if in_class {
						"Invalid class escape".into()
					} else {
						"Invalid decimal escape".into()
					});
				}
				self.pos -= 1;
				Ok(self.legacy_octal())
			}
			'x' => {
				if let Some(value) = self.hex_digits(2) {
					return Ok(value);
				}
				if self.unicode {
					return Err("Invalid escape".into());
				}
				Ok(u32::from('x'))
			}
			'u' => {
				if let Some(value) = self.unicode_escape()? {
					return Ok(value);
				}
				if self.unicode {
					return Err("Invalid Unicode escape".into());
				}
				Ok(u32::from('u'))
			}
			'-' if in_class => Ok(u32::from('-')),
			_ => {
				if self.unicode {
					if "^$\\.*+?()[]{}|/".contains(c) {
						return Ok(u32::from(c));
					}
					return Err("Invalid escape".into());
				}
				if c == 'k' && !self.names.is_empty() {
					return Err("Invalid named reference".into());
				}
				Ok(u32::from(c))
			}
		}
	}

	// `n` hex digits at the current position, consumed when all are there.
	fn hex_digits(&mut self, n: usize) -> Option<u32> {
		let mut value = 0;
		for i in 0..n {
			value = value * 16 + self.peek_at(i)?.to_digit(16)?;
		}
		self.pos += n;
		Some(value)
	}

	// After `\u`: four hex digits (a surrogate pair of two such escapes is one code point), or with `u` `{hex}`.
	fn unicode_escape(&mut self) -> Result<Option<u32>, String> {
		if self.unicode && self.peek() == Some('{') {
			let mut i = 1;
			let mut value: u32 = 0;
			while let Some(d) = self.peek_at(i).and_then(|d| d.to_digit(16)) {
				value = value.saturating_mul(16).saturating_add(d);
				i += 1;
			}
			if i == 1 || self.peek_at(i) != Some('}') || value > 0x10ffff {
				return Err("Invalid Unicode escape".into());
			}
			self.pos += i + 1;
			return Ok(Some(value));
		}
		let Some(high) = self.hex_digits(4) else {
			return Ok(None);
		};
		if (0xd800..=0xdbff).contains(&high) && self.peek() == Some('\\') && self.peek_at(1) == Some('u') {
			let save = self.pos;
			self.pos += 2;
			match self.hex_digits(4) {
				Some(low) if (0xdc00..=0xdfff).contains(&low) => {
					return Ok(Some(0x10000 + ((high - 0xd800) << 10) + (low - 0xdc00)));
				}
				_ => self.pos = save,
			}
		}
		Ok(Some(high))
	}

	fn class(&mut self) -> Result<(), String> {
		self.pos += 1;
		let negated = self.eat('^');
		let mut items = String::new();
		let mut empty = true;
		loop {
			let Some(c) = self.peek() else {
				return Err("Unterminated character class".into());
			};
			if c == ']' {
				self.pos += 1;
				break;
			}
			let first = self.class_atom()?;
			if self.peek() == Some('-') && self.peek_at(1).is_some_and(|next| next != ']') {
				self.pos += 1;
				let second = self.class_atom()?;
				match (first, second) {
					(ClassAtom::Char(lo), ClassAtom::Char(hi)) => {
						if lo > hi {
							return Err("Range out of order in character class".into());
						}
						push_range(&mut items, lo, hi);
					}
					(first, second) => {
						if self.unicode {
							return Err("Invalid character class".into());
						}
						push_class_atom(&mut items, &first);
						push_range(&mut items, u32::from('-'), u32::from('-'));
						push_class_atom(&mut items, &second);
					}
				}
			} else {
				push_class_atom(&mut items, &first);
			}
			empty = false;
		}
		match (empty || items.is_empty(), negated) {
			// `[]` matches nothing and `[^]` any character.
			(true, false) => self.out.push_str("(?!)"),
			(true, true) => self.out.push_str("(?s:.)"),
			(false, false) => self.out.push_str(&format!("[{items}]")),
			(false, true) => self.out.push_str(&format!("[^{items}]")),
		}
		Ok(())
	}

	fn class_atom(&mut self) -> Result<ClassAtom, String> {
		let c = self.peek().expect("class_atom() runs inside a class");
		self.pos += 1;
		if c != '\\' {
			return Ok(ClassAtom::Char(u32::from(c)));
		}
		if self.peek().is_none() {
			return Err("\\ at end of pattern".into());
		}
		if self.peek() == Some('b') {
			self.pos += 1;
			return Ok(ClassAtom::Char(0x08));
		}
		if !self.unicode && self.peek() == Some('k') {
			self.pos += 1;
			return Ok(ClassAtom::Char(u32::from('k')));
		}
		if let Some(escape) = self.class_escape()? {
			return Ok(ClassAtom::Escape(escape));
		}
		Ok(ClassAtom::Char(self.character_escape(true)?))
	}

	// A literal code point (a lone surrogate never matches: a Rust string cannot hold one).
	fn literal(&mut self, value: u32) {
		match char::from_u32(value) {
			Some(c) if c.is_ascii_alphanumeric() || c == '_' || c == ' ' => self.out.push(c),
			Some(_) => self.out.push_str(&format!("\\x{{{value:X}}}")),
			None => self.out.push_str("(?!)"),
		}
	}
}

fn push_class_atom(items: &mut String, atom: &ClassAtom) {
	match atom {
		ClassAtom::Char(value) => push_range(items, *value, *value),
		ClassAtom::Escape(escape) => items.push_str(&escape.items()),
	}
}

// JavaScript's `\b` and `\B`: boundaries between ASCII word characters and the rest.
const WORD_BOUNDARY: &str = "(?:(?<=[0-9A-Za-z_])(?![0-9A-Za-z_])|(?<![0-9A-Za-z_])(?=[0-9A-Za-z_]))";
const NOT_WORD_BOUNDARY: &str = "(?:(?<=[0-9A-Za-z_])(?=[0-9A-Za-z_])|(?<![0-9A-Za-z_])(?![0-9A-Za-z_]))";

// The number of capturing groups and the names of the named ones, counted before parsing (a backreference may
// precede its group).
fn scan_groups(chars: &[char]) -> (usize, Vec<String>) {
	let mut groups = 0;
	let mut names = Vec::new();
	let mut i = 0;
	let mut in_class = false;
	while i < chars.len() {
		match chars[i] {
			'\\' => i += 1,
			'[' => in_class = true,
			']' => in_class = false,
			'(' if !in_class => {
				if chars.get(i + 1) != Some(&'?') {
					groups += 1;
				} else if chars.get(i + 2) == Some(&'<') && !matches!(chars.get(i + 3), Some('=') | Some('!')) {
					groups += 1;
					let mut name = String::new();
					let mut j = i + 3;
					while j < chars.len() && chars[j] != '>' {
						name.push(chars[j]);
						j += 1;
					}
					names.push(name);
				}
			}
			_ => {}
		}
		i += 1;
	}
	(groups, names)
}

// IdentifierStart / IdentifierPart of a group name.
fn is_name_char(c: char, first: bool) -> bool {
	if c == '$' || c == '_' {
		return true;
	}
	if first {
		c.is_alphabetic()
	} else {
		c.is_alphanumeric() || c == '\u{200c}' || c == '\u{200d}'
	}
}

// fancy_regex group names are ASCII words: other names are encoded.
fn rust_group_name(name: &str) -> String {
	if name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') && !name.starts_with(|c: char| c.is_ascii_digit()) {
		return name.to_string();
	}
	let mut out = String::from("g");
	for c in name.chars() {
		out.push_str(&format!("_{:x}", u32::from(c)));
	}
	out
}

// The shape ECMAScript allows in `\p{...}`, and a name the Rust Unicode tables know.
fn valid_property(name: &str) -> bool {
	let value = match name.split_once('=') {
		Some((key, value)) => {
			if general_category_alias(value).is_some() {
				return matches!(key, "General_Category" | "gc");
			}
			if !matches!(
				key,
				"General_Category" | "gc" | "Script" | "sc" | "Script_Extensions" | "scx"
			) {
				return false;
			}
			value
		}
		None if general_category_alias(name).is_some() => return true,
		None => name,
	};
	if value.is_empty() || value.contains('=') || !value.starts_with(|c: char| c.is_ascii_uppercase()) {
		return false;
	}
	::regex::Regex::new(&format!("\\p{{{name}}}")).is_ok()
}

// The General_Category value aliases that start in lowercase (`\p{digit}` is `\p{Nd}`), as the Rust category they name.
fn general_category_alias(value: &str) -> Option<&'static str> {
	match value {
		"digit" => Some("Nd"),
		"punct" => Some("P"),
		"cntrl" => Some("Cc"),
		_ => None,
	}
}
