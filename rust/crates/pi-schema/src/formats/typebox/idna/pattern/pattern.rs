//! Port of `packages/schema/src/formats/typebox/idna/pattern/pattern.ts`.
//! PORT-STATUS: ported
//!
//! Rust strings hold Unicode scalar values only, so `RE_HIGH_SURROGATE` can never match, and a pattern without the
//! `u` flag sees an astral character as one character where JavaScript sees two UTF-16 units: `RE_RULE_NOT_RESERVED_ACE`
//! (`..` is two units in JavaScript) differs on a label whose first character is astral, which `RE_ASCII_LDH` rejects
//! anyway. JavaScript's `.` is spelled `[^\n\r\u{2028}\u{2029}]`. The lookaround rules need `fancy_regex`. The
//! `\p{...}` classes use the Rust Unicode tables (Unicode 16), while Node 24 uses Unicode 17: a character new in 17
//! is not a letter, mark or digit here.

// /*--------------------------------------------------------------------------
//
// TypeBox
//
// The MIT License (MIT)
//
// Copyright (c) 2017-2026 Haydn Paterson
//
// Permission is hereby granted, free of charge, to any person obtaining a copy
// of this software and associated documentation files (the "Software"), to deal
// in the Software without restriction, including without limitation the rights
// to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
// copies of the Software, and to permit persons to whom the Software is
// furnished to do so, subject to the following conditions:
//
// The above copyright notice and this permission notice shall be included in
// all copies or substantial portions of the Software.
//
// THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
// IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
// FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
// AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
// LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
// OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN
// THE SOFTWARE.
//
// ---------------------------------------------------------------------------*/
use std::sync::LazyLock;

use ::regex::Regex;

// -------------------------------------------------------------------
// Validation Rules
// -------------------------------------------------------------------
pub static RE_RULE_HYPHEN_PLACEMENT: LazyLock<fancy_regex::Regex> = LazyLock::new(|| {
	fancy_regex::Regex::new(r"^(?!-)[^\n\r\x{2028}\x{2029}]*(?<!-)$")
		.expect("RE_RULE_HYPHEN_PLACEMENT is a valid pattern")
});
pub static RE_RULE_NOT_RESERVED_ACE: LazyLock<fancy_regex::Regex> = LazyLock::new(|| {
	fancy_regex::Regex::new(r"^(?![^\n\r\x{2028}\x{2029}]{2}--)").expect("RE_RULE_NOT_RESERVED_ACE is a valid pattern")
});
pub static RE_ASCII_LDH: LazyLock<Regex> =
	LazyLock::new(|| Regex::new(r"^[a-zA-Z0-9-]*$").expect("RE_ASCII_LDH is a valid pattern"));

// -------------------------------------------------------------------
// ASCII & Encoding
// -------------------------------------------------------------------
pub static RE_ASCII: LazyLock<Regex> =
	LazyLock::new(|| Regex::new(r"^\p{ASCII}*$").expect("RE_ASCII is a valid pattern"));
pub static RE_NON_ASCII: LazyLock<Regex> =
	LazyLock::new(|| Regex::new(r"[^\p{ASCII}]").expect("RE_NON_ASCII is a valid pattern"));

// -------------------------------------------------------------------
// Structural & Unicode Controls
// -------------------------------------------------------------------
// A Rust string holds no surrogate code point, so this class is empty and never matches.
pub static RE_HIGH_SURROGATE: LazyLock<Regex> =
	LazyLock::new(|| Regex::new(r"[^\x{0}-\x{10ffff}]").expect("RE_HIGH_SURROGATE is a valid pattern"));
pub static RE_REGIONAL_INDICATOR: LazyLock<Regex> =
	LazyLock::new(|| Regex::new(r"[\x{1f1e6}-\x{1f1ff}]").expect("RE_REGIONAL_INDICATOR is a valid pattern"));
pub static RE_VARIATION_SELECTOR: LazyLock<Regex> =
	LazyLock::new(|| Regex::new(r"[\x{fe00}-\x{fe0f}]").expect("RE_VARIATION_SELECTOR is a valid pattern"));
pub static RE_ZERO_WIDTH_JOINER: LazyLock<Regex> =
	LazyLock::new(|| Regex::new(r"\x{200d}").expect("RE_ZERO_WIDTH_JOINER is a valid pattern"));

// -------------------------------------------------------------------
// Numbers & Digits
// -------------------------------------------------------------------
pub static RE_ASCII_DIGIT: LazyLock<Regex> =
	LazyLock::new(|| Regex::new(r"[0-9]").expect("RE_ASCII_DIGIT is a valid pattern"));
pub static RE_ARABIC_INDIC_DIGIT: LazyLock<Regex> =
	LazyLock::new(|| Regex::new(r"[\x{0660}-\x{0669}]").expect("RE_ARABIC_INDIC_DIGIT is a valid pattern"));
pub static RE_EXT_ARABIC_INDIC_DIGIT: LazyLock<Regex> =
	LazyLock::new(|| Regex::new(r"[\x{06f0}-\x{06f9}]").expect("RE_EXT_ARABIC_INDIC_DIGIT is a valid pattern"));

// -------------------------------------------------------------------
// Separators
// -------------------------------------------------------------------
pub static RE_COMMON_SEPARATOR: LazyLock<Regex> = LazyLock::new(|| {
	Regex::new(r"[\x{002e}\x{002c}\x{003a}\x{002f}]").expect("RE_COMMON_SEPARATOR is a valid pattern")
});
pub static RE_EUROPEAN_SEPARATOR: LazyLock<Regex> =
	LazyLock::new(|| Regex::new(r"[\x{002d}\x{002b}]").expect("RE_EUROPEAN_SEPARATOR is a valid pattern"));

// -------------------------------------------------------------------
// General Categories
// -------------------------------------------------------------------
pub static RE_MARK_NONSPACING: LazyLock<Regex> =
	LazyLock::new(|| Regex::new(r"\p{Mn}").expect("RE_MARK_NONSPACING is a valid pattern"));
pub static RE_MARK_SPACING_COMBINING: LazyLock<Regex> =
	LazyLock::new(|| Regex::new(r"\p{Mc}").expect("RE_MARK_SPACING_COMBINING is a valid pattern"));
pub static RE_MARK_ENCLOSING: LazyLock<Regex> =
	LazyLock::new(|| Regex::new(r"\p{Me}").expect("RE_MARK_ENCLOSING is a valid pattern"));
pub static RE_COMBINING_MARK: LazyLock<Regex> =
	LazyLock::new(|| Regex::new(r"[\p{Mn}\p{Mc}\p{Me}]").expect("RE_COMBINING_MARK is a valid pattern"));
pub static RE_LETTER: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\p{L}").expect("RE_LETTER is a valid pattern"));
pub static RE_NUMBER_DECIMAL: LazyLock<Regex> =
	LazyLock::new(|| Regex::new(r"\p{Nd}").expect("RE_NUMBER_DECIMAL is a valid pattern"));

// -------------------------------------------------------------------
// Scripts
// -------------------------------------------------------------------
pub static RE_SCRIPT_GREEK: LazyLock<Regex> =
	LazyLock::new(|| Regex::new(r"\p{Script=Greek}").expect("RE_SCRIPT_GREEK is a valid pattern"));
pub static RE_SCRIPT_HEBREW: LazyLock<Regex> =
	LazyLock::new(|| Regex::new(r"\p{Script=Hebrew}").expect("RE_SCRIPT_HEBREW is a valid pattern"));
pub static RE_SCRIPT_JAPANESE: LazyLock<Regex> = LazyLock::new(|| {
	Regex::new(r"[\p{Script=Hiragana}\p{Script=Katakana}\p{Script=Han}]")
		.expect("RE_SCRIPT_JAPANESE is a valid pattern")
});
pub static RE_SCRIPT_ARABIC_LETTER: LazyLock<Regex> = LazyLock::new(|| {
	Regex::new(r"[\p{Script=Arabic}\p{Script=Syriac}\p{Script=Thaana}\p{Script=Mandaic}]")
		.expect("RE_SCRIPT_ARABIC_LETTER is a valid pattern")
});

// -------------------------------------------------------------------
// RFC Specifications & Context Exceptions
// -------------------------------------------------------------------
pub static RE_VIRAMA: LazyLock<Regex> = LazyLock::new(|| {
	Regex::new(r"[\x{094d}\x{09cd}\x{0a4d}\x{0acd}\x{0b4d}\x{0bcd}\x{0c4d}\x{0ccd}\x{0d3b}\x{0d3c}\x{0d4d}\x{0dca}\x{1b44}\x{1baa}\x{1bab}\x{a9c0}\x{11046}\x{1107f}\x{110b9}\x{11133}\x{11134}\x{111c0}\x{11235}\x{1134d}\x{11442}\x{114c2}\x{115bf}\x{1163f}\x{116b6}\x{11c3f}\x{11d44}\x{11d45}]")
		.expect("RE_VIRAMA is a valid pattern")
});
pub static RE_RFC5892_DISALLOWED: LazyLock<Regex> = LazyLock::new(|| {
	Regex::new(r"[\x{0640}\x{07fa}\x{302e}\x{302f}\x{3031}\x{3032}\x{3033}\x{3034}\x{3035}\x{303b}]")
		.expect("RE_RFC5892_DISALLOWED is a valid pattern")
});
pub static RE_CONTEXTO_EXCEPTIONS: LazyLock<Regex> = LazyLock::new(|| {
	Regex::new(r"[\x{00b7}\x{0375}\x{05f3}\x{05f4}\x{200c}\x{200d}\x{30fb}]")
		.expect("RE_CONTEXTO_EXCEPTIONS is a valid pattern")
});
pub static RE_PVALID_EXCEPTIONS: LazyLock<Regex> = LazyLock::new(|| {
	Regex::new(r"[\x{00df}\x{03c2}\x{06fd}\x{06fe}\x{0f0b}\x{3007}]").expect("RE_PVALID_EXCEPTIONS is a valid pattern")
});

// -------------------------------------------------------------------
// Composite Patterns
// -------------------------------------------------------------------
pub static RE_EUROPEAN_NUMBER: LazyLock<Regex> = LazyLock::new(|| {
	Regex::new(
		&[&RE_ASCII_DIGIT, &RE_EXT_ARABIC_INDIC_DIGIT]
			.map(|regexp| regexp.as_str())
			.join("|"),
	)
	.expect("RE_EUROPEAN_NUMBER is a valid pattern")
});

pub static RE_PERMITTED_CATEGORY: LazyLock<Regex> = LazyLock::new(|| {
	Regex::new(
		&[
			&RE_LETTER,
			&RE_EUROPEAN_SEPARATOR,
			&RE_COMMON_SEPARATOR,
			&RE_NUMBER_DECIMAL,
			&RE_MARK_NONSPACING,
			&RE_MARK_SPACING_COMBINING,
			&RE_CONTEXTO_EXCEPTIONS,
			&RE_PVALID_EXCEPTIONS,
		]
		.map(|regexp| regexp.as_str())
		.join("|"),
	)
	.expect("RE_PERMITTED_CATEGORY is a valid pattern")
});
