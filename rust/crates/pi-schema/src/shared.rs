//! Port of `packages/schema/src/shared.ts`.
//! PORT-STATUS: ported
//!
//! Regular expressions shared across all compiled schemas while one still uses them: tool schemas repeat a few
//! patterns many times, and a RegExp is large once it has run. The cache holds its values weakly and drops an entry
//! once its RegExp is dropped, so it keeps nothing alive.
//!
//! Rust-only: [`JsRegex`] stands in for the ECMAScript `RegExp`. It parses the ECMAScript pattern syntax (with and
//! without the `u` flag, including the Annex B rules that apply without it), rejects what `new RegExp` rejects, and
//! translates the pattern to `fancy_regex` syntax with JavaScript's meaning of `.`, `\d`, `\w`, `\s`, `\b`, `^`/`$`
//! with `m`, and backreferences (by number, empty when the group did not participate). Like V8 it never gives up
//! on a slow match. Known gaps:
//! - Patterns run on Unicode scalar values, so lone surrogates cannot occur and a non-`u` pattern sees an astral
//!   character as one character, not two UTF-16 units.
//! - A quantifier does not reset the groups inside it on each iteration, so a backreference can see the capture of
//!   an earlier iteration: `^(?:(a)|b)+\1$` does not match `"ab"`.
//! - `\p{...}` names are checked against ECMAScript's list (V8, Unicode 17) but match by the Rust Unicode tables
//!   (Unicode 16); `Changes_When_NFKC_Casefolded` is not in them, so a pattern using it does not compile.
//! - With `i`, `\P{...}` is the complement of the case-closed property, where JavaScript case-closes the complement:
//!   `/^\P{Lu}$/ui` does not match `"A"`.
//! - Case-insensitive matching uses Unicode simple case folding in both modes.
//! - Where V8 departs from the specification, the port follows the specification: V8 accepts a group name repeated
//!   inside the first alternative of a group of that name (`(?<n>(?<n>x)|y)`), and with `u` an earlier
//!   `(?i:...)` group makes a later `\w` case-insensitive (`/(?i:)\w/u` matches "\u{212a}").
//!
//! Error messages follow V8's `Invalid regular expression: /source/flags: reason` form; a pattern the Rust engine
//! cannot build reports the engine's reason.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::{Rc, Weak};
use std::sync::LazyLock;

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

// Counted repetitions expand into the compiled program, so a bound such as `{1,100000}` needs room V8 never needs.
const DELEGATE_SIZE_LIMIT: usize = 256 << 20;

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
		let mut multiline = false;
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
				'm' => multiline = true,
				's' => dot_all = true,
				_ => {}
			}
		}
		// V8 names the flags in its canonical order.
		let ordered: String = "dgimsuy".chars().filter(|flag| seen.contains(*flag)).collect();
		let fail = |reason: &str| {
			PiError::named(
				"SyntaxError",
				format!("Invalid regular expression: /{source}/{ordered}: {reason}"),
			)
		};
		let translated = Translator::run(source, unicode, dot_all, multiline).map_err(|reason| fail(&reason))?;
		let pattern = if prefix.is_empty() {
			translated
		} else {
			format!("(?{prefix}){translated}")
		};
		let regex = fancy_regex::RegexBuilder::new(&pattern)
			// V8 never gives up on a match, however long it takes.
			.backtrack_limit(usize::MAX)
			.delegate_size_limit(DELEGATE_SIZE_LIMIT)
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
	/// Script=Unknown: the unassigned and private-use code points.
	UnknownScript(bool),
}

impl ClassEscape {
	// The class items of this escape, for use inside a bracketed class.
	fn items(&self) -> String {
		match self {
			ClassEscape::Ranges(ranges, false) => ranges_text(ranges),
			ClassEscape::Ranges(ranges, true) => format!("[^{}]", ranges_text(ranges)),
			ClassEscape::Property(name, false) => format!("\\p{{{name}}}"),
			ClassEscape::Property(name, true) => format!("\\P{{{name}}}"),
			ClassEscape::UnknownScript(false) => "[\\p{Cn}\\p{Co}]".into(),
			ClassEscape::UnknownScript(true) => "[^\\p{Cn}\\p{Co}]".into(),
		}
	}
}

// `\p{name}` (or `\P` when negated) for a name valid_property() accepted, in the terms of the Rust Unicode tables.
fn property_escape(name: String, negated: bool) -> ClassEscape {
	let value = name.rsplit('=').next().expect("rsplit yields at least one part");
	match value {
		// A Rust string holds no surrogates.
		"Cs" | "Surrogate" => ClassEscape::Ranges(if negated { ALL_RANGES } else { &[] }, false),
		"Unknown" | "Zzzz" => ClassEscape::UnknownScript(negated),
		// Scripts new in Unicode 17, whose characters the Rust tables (Unicode 16) do not assign.
		"Berf" | "Beria_Erfe" | "Sidt" | "Sidetic" | "Tayo" | "Tai_Yo" | "Tols" | "Tolong_Siki" => {
			ClassEscape::Ranges(if negated { ALL_RANGES } else { &[] }, false)
		}
		_ => match general_category_alias(value) {
			Some(category) => ClassEscape::Property(category.to_string(), negated),
			None => ClassEscape::Property(name, negated),
		},
	}
}

const ALL_RANGES: &[(u32, u32)] = &[(0, 0x10ffff)];

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

// V8's maximum string length (2^29 - 24 UTF-16 units): no repetition bound above it can make a difference.
const MAX_STRING_LENGTH: u64 = 536_870_888;

/// The parser of an ECMAScript pattern that writes its fancy_regex translation.
struct Translator {
	chars: Vec<char>,
	pos: usize,
	unicode: bool,
	/// The number of capturing groups and the named ones with their numbers, from a scan before parsing.
	groups: usize,
	named: Vec<(String, usize)>,
	/// Whether the pattern has a `(?<` group, even one with an invalid name: V8 then reads `\k` as a reference.
	named_groups: bool,
	dot_all: Vec<bool>,
	multiline: Vec<bool>,
	/// The names of the named groups closed so far.
	defined: Vec<String>,
	/// The number of capturing groups opened so far, the ones still open, and the lookbehinds around the position.
	opened: usize,
	open: Vec<usize>,
	lookbehinds: usize,
	/// A `\k<name>` without a group of that name: V8 reports it only once the whole pattern parsed.
	unknown_reference: bool,
	out: String,
}

/// What a term was: an assertion that may not be quantified, a lookbehind (an assertion V8 reports differently), a
/// lookahead (quantifiable without `u`), an atom that never consumes a character (a group of assertions, an empty
/// group, a reference that is always empty), or an atom.
enum Term {
	Assertion,
	Lookbehind,
	Lookahead,
	ZeroWidth,
	Atom,
}

impl Translator {
	fn run(source: &str, unicode: bool, dot_all: bool, multiline: bool) -> Result<String, String> {
		let chars: Vec<char> = source.chars().collect();
		let (groups, named, named_groups) = scan_groups(&chars);
		let mut t = Translator {
			chars,
			pos: 0,
			unicode,
			groups,
			named,
			named_groups,
			dot_all: vec![dot_all],
			multiline: vec![multiline],
			defined: Vec::new(),
			opened: 0,
			open: Vec::new(),
			lookbehinds: 0,
			unknown_reference: false,
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
		if t.unknown_reference {
			return Err("Invalid named capture referenced".into());
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

	// Each alternative sees only the names defined before the disjunction: a name may repeat in another alternative,
	// since both groups can never participate in one match. Returns whether no alternative can consume a character.
	fn disjunction(&mut self) -> Result<bool, String> {
		let before = self.defined.len();
		let mut added = Vec::new();
		let mut zero_width = true;
		loop {
			zero_width &= self.alternative()?;
			added.extend(self.defined.drain(before..));
			if !self.eat('|') {
				break;
			}
			self.out.push('|');
		}
		self.defined.extend(added);
		Ok(zero_width)
	}

	// Returns whether the alternative cannot consume a character.
	fn alternative(&mut self) -> Result<bool, String> {
		let mut zero_width = true;
		while let Some(c) = self.peek() {
			if c == '|' || c == ')' {
				break;
			}
			let start = self.out.len();
			let term = self.term()?;
			zero_width &= !matches!(term, Term::Atom);
			self.quantifier(term, start)?;
		}
		Ok(zero_width)
	}

	fn term(&mut self) -> Result<Term, String> {
		let c = self.peek().expect("term() runs before the end of the pattern");
		match c {
			'^' | '$' => {
				self.pos += 1;
				// With `m`, JavaScript's lines also end at \r, U+2028 and U+2029, not only at \n.
				let multiline = *self.multiline.last().expect("the multiline stack is never empty");
				match (c, multiline) {
					('^', true) => self.out.push_str("(?:^|(?<=[\\n\\r\\x{2028}\\x{2029}]))"),
					(_, true) => self.out.push_str("(?:$|(?=[\\n\\r\\x{2028}\\x{2029}]))"),
					_ => self.out.push(c),
				}
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
				Some(_) if matches!(term, Term::Assertion) => return Err("Nothing to repeat".into()),
				Some((min, max, len)) => {
					if max.is_some_and(|max| max < min) {
						return Err("numbers out of order in {} quantifier".into());
					}
					self.pos += len - 1;
					// No string is longer than V8's maximum, so a larger bound is no bound (and one beyond the
					// regex crate's counts would not compile).
					let text = match max.filter(|max| *max <= MAX_STRING_LENGTH) {
						_ if min > MAX_STRING_LENGTH => String::new(),
						Some(max) if max == min => format!("{{{min}}}"),
						Some(max) => format!("{{{min},{max}}}"),
						None => format!("{{{min},}}"),
					};
					(min, text)
				}
				None if self.unicode && !matches!(term, Term::Assertion) => return Err("Incomplete quantifier".into()),
				None => return Ok(()),
			},
			_ => return Ok(()),
		};
		self.pos += 1;
		let lazy = self.eat('?');
		match term {
			Term::Assertion => return Err("Nothing to repeat".into()),
			Term::Lookbehind => return Err("Invalid quantifier".into()),
			Term::Lookahead if self.unicode => return Err("Invalid quantifier".into()),
			// JavaScript rejects an iteration that matched the empty string once the minimum is met, so an atom that
			// never consumes runs once when it must repeat and not at all otherwise (fancy_regex cannot repeat it).
			// The atom stays for its group numbers.
			Term::Lookahead | Term::ZeroWidth => {
				if min == 0 {
					let atom = self.out.split_off(start);
					self.out.push_str(&format!("(?:|{NOTHING}{atom})"));
				}
				return Ok(());
			}
			Term::Atom => {}
		}
		let atom = self.out.split_off(start);
		// A minimum no string can reach never matches; the atom stays for its group numbers.
		if text.is_empty() {
			self.out.push_str(&format!("{NOTHING}(?:{atom})"));
			return Ok(());
		}
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
			return self.capturing_group();
		}
		match self.peek() {
			Some(':') => {
				self.pos += 1;
				self.out.push_str("(?:");
				self.group_body()
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
				self.lookbehinds += 1;
				self.disjunction()?;
				self.close_group()?;
				self.lookbehinds -= 1;
				Ok(Term::Lookbehind)
			}
			Some('<') => {
				self.pos += 1;
				let name = self.group_name()?;
				// A plain group: `\k<name>` becomes its number (see atom_escape). Like V8, the name counts once the
				// group closes.
				let term = self.capturing_group()?;
				if self.defined.contains(&name) {
					return Err("Duplicate capture group name".into());
				}
				self.defined.push(name);
				Ok(term)
			}
			_ => self.modifiers(),
		}
	}

	// A capturing group after its opening, through the closing parenthesis.
	fn capturing_group(&mut self) -> Result<Term, String> {
		self.opened += 1;
		self.open.push(self.opened);
		self.out.push('(');
		let term = self.group_body()?;
		self.open.pop();
		Ok(term)
	}

	// The disjunction of a group and its closing parenthesis: an atom, or a zero-width one when it cannot consume.
	fn group_body(&mut self) -> Result<Term, String> {
		let zero_width = self.disjunction()?;
		self.close_group()?;
		Ok(if zero_width { Term::ZeroWidth } else { Term::Atom })
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
						return Err("Repeated flag in flag group".into());
					}
					if removing {
						remove.push(c)
					} else {
						add.push(c)
					}
					self.pos += 1;
				}
				Some('-') if removing => return Err("Multiple dashes in flag group".into()),
				Some('-') => {
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
			return Err("Invalid flag group".into());
		}
		if !removing && add.is_empty() {
			return Err("Invalid group".into());
		}
		let flags: String = add.chars().filter(|c| *c == 'i').collect();
		let mut flags = flags;
		let negative: String = remove.chars().filter(|c| *c == 'i').collect();
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
		let multiline = if add.contains('m') {
			true
		} else if remove.contains('m') {
			false
		} else {
			*self.multiline.last().expect("the multiline stack is never empty")
		};
		self.multiline.push(multiline);
		if flags.is_empty() {
			self.out.push_str("(?:");
		} else {
			self.out.push_str(&format!("(?{flags}:"));
		}
		let term = self.group_body()?;
		self.dot_all.pop();
		self.multiline.pop();
		Ok(term)
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
		read_group_name(&self.chars, &mut self.pos).ok_or_else(|| "Invalid capture group name".into())
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
					if !self.participated(n) {
						return Ok(Term::ZeroWidth);
					}
					self.out.push_str(&backreference(n));
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
			'k' if self.unicode || self.named_groups => {
				self.pos += 1;
				if !self.eat('<') {
					return Err("Invalid named reference".into());
				}
				let name = self.group_name()?;
				// By number, so a reference may precede its group; a name used in several alternatives refers to
				// whichever group participated.
				let numbers: Vec<usize> = self
					.named
					.iter()
					.filter(|(known, _)| *known == name)
					.map(|(_, number)| *number)
					.collect();
				if numbers.is_empty() {
					self.unknown_reference = true;
					return Ok(Term::Atom);
				}
				let references: Vec<String> = numbers
					.into_iter()
					.filter(|n| self.participated(*n))
					.map(backreference)
					.collect();
				if references.is_empty() {
					return Ok(Term::ZeroWidth);
				}
				self.out.push_str(&format!("(?:{})", references.concat()));
				return Ok(Term::Atom);
			}
			_ => {}
		}
		if let Some(escape) = self.class_escape()? {
			let items = escape.items();
			if items.is_empty() {
				self.out.push_str(NOTHING);
			} else {
				self.out.push('[');
				self.out.push_str(&items);
				self.out.push(']');
			}
			return Ok(Term::Atom);
		}
		let value = self.character_escape(false)?;
		self.literal(value);
		Ok(Term::Atom)
	}

	// Whether group `n` can have participated when a reference at the current position runs. A reference inside its own
	// group or before it is always empty in JavaScript, since a quantifier clears the captures of its atom on each
	// iteration; in a lookbehind, which matches backwards, the reference stays.
	fn participated(&self, n: usize) -> bool {
		self.lookbehinds > 0 || (n <= self.opened && !self.open.contains(&n))
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
				return Ok(Some(property_escape(name, c == 'P')));
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
					return Err("Invalid Unicode escape".into());
				}
				// Annex B: "\c" not followed by a control letter is a backslash and a "c".
				self.pos -= 1;
				Ok(u32::from('\\'))
			}
			'0' if !self.peek().is_some_and(|d| d.is_ascii_digit()) => Ok(0),
			'0'..='9' => {
				if self.unicode {
					return Err(if c >= '8' {
						"Invalid escape".into()
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
				if c == 'k' && self.named_groups {
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
			(true, false) => self.out.push_str(NOTHING),
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
			if self.named_groups {
				return Err("Invalid escape".into());
			}
			self.pos += 1;
			return Ok(ClassAtom::Char(u32::from('k')));
		}
		let escape = self.class_escape().map_err(|error| {
			if error == "Invalid property name" {
				"Invalid property name in character class".into()
			} else {
				error
			}
		})?;
		if let Some(escape) = escape {
			return Ok(ClassAtom::Escape(escape));
		}
		Ok(ClassAtom::Char(self.character_escape(true)?))
	}

	// A literal code point (a lone surrogate never matches: a Rust string cannot hold one).
	fn literal(&mut self, value: u32) {
		match char::from_u32(value) {
			Some(c) if c.is_ascii_alphanumeric() || c == '_' || c == ' ' => self.out.push(c),
			Some(_) => self.out.push_str(&format!("\\x{{{value:X}}}")),
			None => self.out.push_str(NOTHING),
		}
	}
}

fn push_class_atom(items: &mut String, atom: &ClassAtom) {
	match atom {
		ClassAtom::Char(value) => push_range(items, *value, *value),
		ClassAtom::Escape(escape) => items.push_str(&escape.items()),
	}
}

// A backreference to group `n`: empty when the group did not participate, as in JavaScript (a conditional, so a
// digit after it does not extend the number).
fn backreference(n: usize) -> String {
	format!("(?({n})\\{n}|)")
}

// A class that matches no character. Unlike `(?!)`, fancy_regex lets it be repeated.
const NOTHING: &str = "[^\\x{0}-\\x{10FFFF}]";

// JavaScript's `\b` and `\B`: boundaries between ASCII word characters and the rest.
const WORD_BOUNDARY: &str = "(?:(?<=[0-9A-Za-z_])(?![0-9A-Za-z_])|(?<![0-9A-Za-z_])(?=[0-9A-Za-z_]))";
const NOT_WORD_BOUNDARY: &str = "(?:(?<=[0-9A-Za-z_])(?=[0-9A-Za-z_])|(?<![0-9A-Za-z_])(?![0-9A-Za-z_]))";

// The number of capturing groups, the named ones with their numbers, and whether any `(?<` group occurs, counted
// before parsing (a backreference may precede its group).
fn scan_groups(chars: &[char]) -> (usize, Vec<(String, usize)>, bool) {
	let mut groups = 0;
	let mut named = Vec::new();
	let mut named_groups = false;
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
					named_groups = true;
					let mut end = i + 3;
					if let Some(name) = read_group_name(chars, &mut end) {
						named.push((name, groups));
					}
				}
			}
			_ => {}
		}
		i += 1;
	}
	(groups, named, named_groups)
}

// The group name at `pos` through its closing `>` (consumed), with its `\u` escapes decoded, or `None` when it is not
// a RegExpIdentifierName.
fn read_group_name(chars: &[char], pos: &mut usize) -> Option<String> {
	let mut name = String::new();
	loop {
		let c = *chars.get(*pos)?;
		*pos += 1;
		match c {
			'>' => break,
			'\\' => {
				if chars.get(*pos) != Some(&'u') {
					return None;
				}
				*pos += 1;
				name.push(char::from_u32(name_escape(chars, pos)?)?);
			}
			c => name.push(c),
		}
	}
	ID_NAME.is_match(&name).then_some(name)
}

// RegExpIdentifierName: ID_Start, `$` or `_`, then ID_Continue, `$`, ZWNJ or ZWJ.
static ID_NAME: LazyLock<regex::Regex> = LazyLock::new(|| {
	regex::Regex::new(r"^[\p{ID_Start}$_][\p{ID_Continue}$\x{200C}\x{200D}]*$").expect("the pattern is valid")
});

// After `\u` in a group name (always with the `u` rules): `{hex}`, four hex digits, or a surrogate pair of two escapes.
fn name_escape(chars: &[char], pos: &mut usize) -> Option<u32> {
	let hex = |pos: &mut usize, n: usize| -> Option<u32> {
		let mut value = 0;
		for i in 0..n {
			value = value * 16 + chars.get(*pos + i)?.to_digit(16)?;
		}
		*pos += n;
		Some(value)
	};
	if chars.get(*pos) == Some(&'{') {
		let mut value: u32 = 0;
		let mut i = *pos + 1;
		while let Some(d) = chars.get(i).and_then(|d| d.to_digit(16)) {
			value = value.saturating_mul(16).saturating_add(d);
			i += 1;
		}
		if i == *pos + 1 || chars.get(i) != Some(&'}') || value > 0x10ffff {
			return None;
		}
		*pos = i + 1;
		return Some(value);
	}
	let high = hex(pos, 4)?;
	if (0xd800..=0xdbff).contains(&high) && chars.get(*pos) == Some(&'\\') && chars.get(*pos + 1) == Some(&'u') {
		let mut after = *pos + 2;
		if let Some(low) = hex(&mut after, 4).filter(|low| (0xdc00..=0xdfff).contains(low)) {
			*pos = after;
			return Some(0x10000 + ((high - 0xd800) << 10) + (low - 0xdc00));
		}
	}
	Some(high)
}

// Whether ECMAScript allows the name in `\p{...}`: its exact names (case-sensitive, no loose matching), as V8 (Unicode
// 17) accepts them.
fn valid_property(name: &str) -> bool {
	match name.split_once('=') {
		Some(("General_Category" | "gc", value)) => GENERAL_CATEGORIES.contains(&value),
		Some(("Script" | "sc" | "Script_Extensions" | "scx", value)) => SCRIPTS.contains(&value),
		Some(_) => false,
		None => LONE_PROPERTIES.contains(&name),
	}
}

// The names allowed alone: binary properties and General_Category values with their aliases.
const LONE_PROPERTIES: [&str; 179] = [
	"AHex",
	"ASCII",
	"ASCII_Hex_Digit",
	"Alpha",
	"Alphabetic",
	"Any",
	"Assigned",
	"Bidi_C",
	"Bidi_Control",
	"Bidi_M",
	"Bidi_Mirrored",
	"C",
	"CI",
	"CWCF",
	"CWCM",
	"CWKCF",
	"CWL",
	"CWT",
	"CWU",
	"Case_Ignorable",
	"Cased",
	"Cased_Letter",
	"Cc",
	"Cf",
	"Changes_When_Casefolded",
	"Changes_When_Casemapped",
	"Changes_When_Lowercased",
	"Changes_When_NFKC_Casefolded",
	"Changes_When_Titlecased",
	"Changes_When_Uppercased",
	"Close_Punctuation",
	"Cn",
	"Co",
	"Combining_Mark",
	"Connector_Punctuation",
	"Control",
	"Cs",
	"Currency_Symbol",
	"DI",
	"Dash",
	"Dash_Punctuation",
	"Decimal_Number",
	"Default_Ignorable_Code_Point",
	"Dep",
	"Deprecated",
	"Dia",
	"Diacritic",
	"EBase",
	"EComp",
	"EMod",
	"EPres",
	"Emoji",
	"Emoji_Component",
	"Emoji_Modifier",
	"Emoji_Modifier_Base",
	"Emoji_Presentation",
	"Enclosing_Mark",
	"Ext",
	"ExtPict",
	"Extended_Pictographic",
	"Extender",
	"Final_Punctuation",
	"Format",
	"Gr_Base",
	"Gr_Ext",
	"Grapheme_Base",
	"Grapheme_Extend",
	"Hex",
	"Hex_Digit",
	"IDC",
	"IDS",
	"IDSB",
	"IDST",
	"IDS_Binary_Operator",
	"IDS_Trinary_Operator",
	"ID_Continue",
	"ID_Start",
	"Ideo",
	"Ideographic",
	"Initial_Punctuation",
	"Join_C",
	"Join_Control",
	"L",
	"LC",
	"LOE",
	"Letter",
	"Letter_Number",
	"Line_Separator",
	"Ll",
	"Lm",
	"Lo",
	"Logical_Order_Exception",
	"Lower",
	"Lowercase",
	"Lowercase_Letter",
	"Lt",
	"Lu",
	"M",
	"Mark",
	"Math",
	"Math_Symbol",
	"Mc",
	"Me",
	"Mn",
	"Modifier_Letter",
	"Modifier_Symbol",
	"N",
	"NChar",
	"Nd",
	"Nl",
	"No",
	"Noncharacter_Code_Point",
	"Nonspacing_Mark",
	"Number",
	"Open_Punctuation",
	"Other",
	"Other_Letter",
	"Other_Number",
	"Other_Punctuation",
	"Other_Symbol",
	"P",
	"Paragraph_Separator",
	"Pat_Syn",
	"Pat_WS",
	"Pattern_Syntax",
	"Pattern_White_Space",
	"Pc",
	"Pd",
	"Pe",
	"Pf",
	"Pi",
	"Po",
	"Private_Use",
	"Ps",
	"Punctuation",
	"QMark",
	"Quotation_Mark",
	"RI",
	"Radical",
	"Regional_Indicator",
	"S",
	"SD",
	"STerm",
	"Sc",
	"Sentence_Terminal",
	"Separator",
	"Sk",
	"Sm",
	"So",
	"Soft_Dotted",
	"Space_Separator",
	"Spacing_Mark",
	"Surrogate",
	"Symbol",
	"Term",
	"Terminal_Punctuation",
	"Titlecase_Letter",
	"UIdeo",
	"Unassigned",
	"Unified_Ideograph",
	"Upper",
	"Uppercase",
	"Uppercase_Letter",
	"VS",
	"Variation_Selector",
	"WSpace",
	"White_Space",
	"XIDC",
	"XIDS",
	"XID_Continue",
	"XID_Start",
	"Z",
	"Zl",
	"Zp",
	"Zs",
	"cntrl",
	"digit",
	"punct",
	"space",
];

// The General_Category values and aliases allowed after `General_Category=` or `gc=`.
const GENERAL_CATEGORIES: [&str; 80] = [
	"C",
	"Cased_Letter",
	"Cc",
	"Cf",
	"Close_Punctuation",
	"Cn",
	"Co",
	"Combining_Mark",
	"Connector_Punctuation",
	"Control",
	"Cs",
	"Currency_Symbol",
	"Dash_Punctuation",
	"Decimal_Number",
	"Enclosing_Mark",
	"Final_Punctuation",
	"Format",
	"Initial_Punctuation",
	"L",
	"LC",
	"Letter",
	"Letter_Number",
	"Line_Separator",
	"Ll",
	"Lm",
	"Lo",
	"Lowercase_Letter",
	"Lt",
	"Lu",
	"M",
	"Mark",
	"Math_Symbol",
	"Mc",
	"Me",
	"Mn",
	"Modifier_Letter",
	"Modifier_Symbol",
	"N",
	"Nd",
	"Nl",
	"No",
	"Nonspacing_Mark",
	"Number",
	"Open_Punctuation",
	"Other",
	"Other_Letter",
	"Other_Number",
	"Other_Punctuation",
	"Other_Symbol",
	"P",
	"Paragraph_Separator",
	"Pc",
	"Pd",
	"Pe",
	"Pf",
	"Pi",
	"Po",
	"Private_Use",
	"Ps",
	"Punctuation",
	"S",
	"Sc",
	"Separator",
	"Sk",
	"Sm",
	"So",
	"Space_Separator",
	"Spacing_Mark",
	"Surrogate",
	"Symbol",
	"Titlecase_Letter",
	"Unassigned",
	"Uppercase_Letter",
	"Z",
	"Zl",
	"Zp",
	"Zs",
	"cntrl",
	"digit",
	"punct",
];

// The Script values and aliases allowed after `Script=`, `sc=`, `Script_Extensions=` or `scx=`.
const SCRIPTS: [&str; 344] = [
	"Adlam",
	"Adlm",
	"Aghb",
	"Ahom",
	"Anatolian_Hieroglyphs",
	"Arab",
	"Arabic",
	"Armenian",
	"Armi",
	"Armn",
	"Avestan",
	"Avst",
	"Bali",
	"Balinese",
	"Bamu",
	"Bamum",
	"Bass",
	"Bassa_Vah",
	"Batak",
	"Batk",
	"Beng",
	"Bengali",
	"Berf",
	"Beria_Erfe",
	"Bhaiksuki",
	"Bhks",
	"Bopo",
	"Bopomofo",
	"Brah",
	"Brahmi",
	"Brai",
	"Braille",
	"Bugi",
	"Buginese",
	"Buhd",
	"Buhid",
	"Cakm",
	"Canadian_Aboriginal",
	"Cans",
	"Cari",
	"Carian",
	"Caucasian_Albanian",
	"Chakma",
	"Cham",
	"Cher",
	"Cherokee",
	"Chorasmian",
	"Chrs",
	"Common",
	"Copt",
	"Coptic",
	"Cpmn",
	"Cprt",
	"Cuneiform",
	"Cypriot",
	"Cypro_Minoan",
	"Cyrillic",
	"Cyrl",
	"Deseret",
	"Deva",
	"Devanagari",
	"Diak",
	"Dives_Akuru",
	"Dogr",
	"Dogra",
	"Dsrt",
	"Dupl",
	"Duployan",
	"Egyp",
	"Egyptian_Hieroglyphs",
	"Elba",
	"Elbasan",
	"Elym",
	"Elymaic",
	"Ethi",
	"Ethiopic",
	"Gara",
	"Garay",
	"Geor",
	"Georgian",
	"Glag",
	"Glagolitic",
	"Gong",
	"Gonm",
	"Goth",
	"Gothic",
	"Gran",
	"Grantha",
	"Greek",
	"Grek",
	"Gujarati",
	"Gujr",
	"Gukh",
	"Gunjala_Gondi",
	"Gurmukhi",
	"Guru",
	"Gurung_Khema",
	"Han",
	"Hang",
	"Hangul",
	"Hani",
	"Hanifi_Rohingya",
	"Hano",
	"Hanunoo",
	"Hatr",
	"Hatran",
	"Hebr",
	"Hebrew",
	"Hira",
	"Hiragana",
	"Hluw",
	"Hmng",
	"Hmnp",
	"Hung",
	"Imperial_Aramaic",
	"Inherited",
	"Inscriptional_Pahlavi",
	"Inscriptional_Parthian",
	"Ital",
	"Java",
	"Javanese",
	"Kaithi",
	"Kali",
	"Kana",
	"Kannada",
	"Katakana",
	"Kawi",
	"Kayah_Li",
	"Khar",
	"Kharoshthi",
	"Khitan_Small_Script",
	"Khmer",
	"Khmr",
	"Khoj",
	"Khojki",
	"Khudawadi",
	"Kirat_Rai",
	"Kits",
	"Knda",
	"Krai",
	"Kthi",
	"Lana",
	"Lao",
	"Laoo",
	"Latin",
	"Latn",
	"Lepc",
	"Lepcha",
	"Limb",
	"Limbu",
	"Lina",
	"Linb",
	"Linear_A",
	"Linear_B",
	"Lisu",
	"Lyci",
	"Lycian",
	"Lydi",
	"Lydian",
	"Mahajani",
	"Mahj",
	"Maka",
	"Makasar",
	"Malayalam",
	"Mand",
	"Mandaic",
	"Mani",
	"Manichaean",
	"Marc",
	"Marchen",
	"Masaram_Gondi",
	"Medefaidrin",
	"Medf",
	"Meetei_Mayek",
	"Mend",
	"Mende_Kikakui",
	"Merc",
	"Mero",
	"Meroitic_Cursive",
	"Meroitic_Hieroglyphs",
	"Miao",
	"Mlym",
	"Modi",
	"Mong",
	"Mongolian",
	"Mro",
	"Mroo",
	"Mtei",
	"Mult",
	"Multani",
	"Myanmar",
	"Mymr",
	"Nabataean",
	"Nag_Mundari",
	"Nagm",
	"Nand",
	"Nandinagari",
	"Narb",
	"Nbat",
	"New_Tai_Lue",
	"Newa",
	"Nko",
	"Nkoo",
	"Nshu",
	"Nushu",
	"Nyiakeng_Puachue_Hmong",
	"Ogam",
	"Ogham",
	"Ol_Chiki",
	"Ol_Onal",
	"Olck",
	"Old_Hungarian",
	"Old_Italic",
	"Old_North_Arabian",
	"Old_Permic",
	"Old_Persian",
	"Old_Sogdian",
	"Old_South_Arabian",
	"Old_Turkic",
	"Old_Uyghur",
	"Onao",
	"Oriya",
	"Orkh",
	"Orya",
	"Osage",
	"Osge",
	"Osma",
	"Osmanya",
	"Ougr",
	"Pahawh_Hmong",
	"Palm",
	"Palmyrene",
	"Pau_Cin_Hau",
	"Pauc",
	"Perm",
	"Phag",
	"Phags_Pa",
	"Phli",
	"Phlp",
	"Phnx",
	"Phoenician",
	"Plrd",
	"Prti",
	"Psalter_Pahlavi",
	"Qaac",
	"Qaai",
	"Rejang",
	"Rjng",
	"Rohg",
	"Runic",
	"Runr",
	"Samaritan",
	"Samr",
	"Sarb",
	"Saur",
	"Saurashtra",
	"Sgnw",
	"Sharada",
	"Shavian",
	"Shaw",
	"Shrd",
	"Sidd",
	"Siddham",
	"Sidetic",
	"Sidt",
	"SignWriting",
	"Sind",
	"Sinh",
	"Sinhala",
	"Sogd",
	"Sogdian",
	"Sogo",
	"Sora",
	"Sora_Sompeng",
	"Soyo",
	"Soyombo",
	"Sund",
	"Sundanese",
	"Sunu",
	"Sunuwar",
	"Sylo",
	"Syloti_Nagri",
	"Syrc",
	"Syriac",
	"Tagalog",
	"Tagb",
	"Tagbanwa",
	"Tai_Le",
	"Tai_Tham",
	"Tai_Viet",
	"Tai_Yo",
	"Takr",
	"Takri",
	"Tale",
	"Talu",
	"Tamil",
	"Taml",
	"Tang",
	"Tangsa",
	"Tangut",
	"Tavt",
	"Tayo",
	"Telu",
	"Telugu",
	"Tfng",
	"Tglg",
	"Thaa",
	"Thaana",
	"Thai",
	"Tibetan",
	"Tibt",
	"Tifinagh",
	"Tirh",
	"Tirhuta",
	"Tnsa",
	"Todhri",
	"Todr",
	"Tolong_Siki",
	"Tols",
	"Toto",
	"Tulu_Tigalari",
	"Tutg",
	"Ugar",
	"Ugaritic",
	"Unknown",
	"Vai",
	"Vaii",
	"Vith",
	"Vithkuqi",
	"Wancho",
	"Wara",
	"Warang_Citi",
	"Wcho",
	"Xpeo",
	"Xsux",
	"Yezi",
	"Yezidi",
	"Yi",
	"Yiii",
	"Zanabazar_Square",
	"Zanb",
	"Zinh",
	"Zyyy",
	"Zzzz",
];

// The General_Category value aliases that start in lowercase (`\p{digit}` is `\p{Nd}`), as the Rust category they name.
fn general_category_alias(value: &str) -> Option<&'static str> {
	match value {
		"digit" => Some("Nd"),
		"punct" => Some("P"),
		"cntrl" => Some("Cc"),
		_ => None,
	}
}
