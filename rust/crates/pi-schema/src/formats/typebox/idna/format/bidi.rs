//! Port of `packages/schema/src/formats/typebox/idna/format/bidi.ts`.
//! PORT-STATUS: ported

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

use super::super::pattern::pattern;
use super::puny as format_puny;

// ------------------------------------------------------------------
// LOCAL REGEX: Allowed Bidi classes per direction (RFC 5893)
// ------------------------------------------------------------------
static RE_RTL_ALLOWED: LazyLock<Regex> =
	LazyLock::new(|| Regex::new(r"^(?:R|AL|AN|EN|ES|CS|ET|ON|BN|NSM)$").expect("RE_RTL_ALLOWED is a valid pattern"));
static RE_LTR_ALLOWED: LazyLock<Regex> =
	LazyLock::new(|| Regex::new(r"^(?:L|EN|ES|CS|ET|ON|BN|NSM)$").expect("RE_LTR_ALLOWED is a valid pattern"));
static RE_RTL_CLASSES: LazyLock<Regex> =
	LazyLock::new(|| Regex::new(r"^(?:R|AL|AN)$").expect("RE_RTL_CLASSES is a valid pattern"));

// ------------------------------------------------------------------
// HasBidiChars
//
// Decodes a punycode label to its Unicode form (if it is one) and
// checks whether the result is an RTL label per RFC 5893 §1.4.
// ------------------------------------------------------------------
pub fn has_bidi_chars(value: &str) -> bool {
	if format_puny::is_ace_prefixed(value) {
		return match format_puny::decode(&value.chars().skip(4).collect::<String>().to_lowercase()) {
			Ok(decoded) => has_right_to_left_characters(&decoded),
			Err(_) => false,
		};
	}
	has_right_to_left_characters(value)
}
// ------------------------------------------------------------------
// GetBidiClass
// ------------------------------------------------------------------
fn get_bidi_class(code_point: char) -> &'static str {
	let char = String::from(code_point);
	if pattern::RE_EUROPEAN_NUMBER.is_match(&char) {
		"EN"
	} else if pattern::RE_ARABIC_INDIC_DIGIT.is_match(&char) {
		"AN"
	// } else if pattern::RE_EUROPEAN_SEPARATOR.is_match(&char) { "ES" // (no-spec-coverage)
	// } else if pattern::RE_COMMON_SEPARATOR.is_match(&char) { "CS" // (no-spec-coverage)
	} else if pattern::RE_MARK_NONSPACING.is_match(&char) {
		"NSM"
	} else if pattern::RE_SCRIPT_HEBREW.is_match(&char) {
		"R"
	} else if pattern::RE_SCRIPT_ARABIC_LETTER.is_match(&char) {
		"AL"
	} else if pattern::RE_LETTER.is_match(&char) {
		"L"
	} else {
		"ON"
	}
}
// ------------------------------------------------------------------
// HasRightToLeftCharacters
//
// Per RFC 5893 §1.4: "An RTL label is a label that contains at least
// one character of type R, AL, or AN." Used to determine whether a
// domain name is a "Bidi domain name" (RFC 5893 §1.4), in which case
// the Bidi Rule applies to every label in the domain.
// ------------------------------------------------------------------
pub fn has_right_to_left_characters(value: &str) -> bool {
	value.chars().any(|ch| RE_RTL_CLASSES.is_match(get_bidi_class(ch)))
}
// ------------------------------------------------------------------
// SatisfiesBidiRule
// ------------------------------------------------------------------
pub fn satisfies_bidi_rule(value: &str) -> bool {
	let mut is_rtl = false;
	let mut allowed: &Regex = &RE_LTR_ALLOWED;
	let mut saw_en = false;
	let mut saw_an = false;
	let mut is_first = true;
	// let lastClass: string | null = null // (no-spec-coverage)
	for ch in value.chars() {
		let bidi_class = get_bidi_class(ch);
		if is_first {
			// 1. The first character must be a character with Bidi property L, R, or AL.
			if bidi_class != "L" && bidi_class != "R" && bidi_class != "AL" {
				return false;
			}
			is_rtl = bidi_class == "R" || bidi_class == "AL";
			allowed = if is_rtl { &RE_RTL_ALLOWED } else { &RE_LTR_ALLOWED };
			is_first = false;
		}
		// 2 / 5. Only characters in the allowed set for this direction may appear.
		if !allowed.is_match(bidi_class) {
			return false;
		}
		if bidi_class == "EN" {
			saw_en = true;
		} else if bidi_class == "AN" {
			saw_an = true;
		}
		// if (bidiClass !== 'NSM') lastClass = bidiClass // (no-spec-coverage)
	}
	if is_rtl && saw_en && saw_an {
		return false; // spec coverage path
	}
	// (no-spec-coverage)
	// if (isRtl) {
	//   // 3. The end of the label must be R, AL, EN, or AN (ignoring trailing NSM).
	//   // if (lastClass !== 'R' && lastClass !== 'AL' && lastClass !== 'EN' && lastClass !== 'AN') return false
	//   // 4. If an EN is present, no AN may be present, and vice versa.
	//   if (sawEN && sawAN) return false
	// } else {
	//   // 6. The end of the label must be L or EN (ignoring trailing NSM).
	//   // if (lastClass !== 'L' && lastClass !== 'EN') return false
	// }
	true
}
