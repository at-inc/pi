//! Port of `packages/schema/src/formats/typebox/idna/label/unicode.ts`.
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
use pi_host::js::utf16_len;

use super::super::format::bidi as format_bidi;
use super::super::format::puny as format_puny;
use super::super::pattern::pattern;

// ------------------------------------------------------------------
// ExceedsMaxALabelLength
//
// RFC 5890 §2.3.2.1: an A-label (the ACE-encoded form of a label,
// "xn--" + punycode) must not exceed 63 octets. Only labels
// containing non-ASCII characters need checking - a pure-ASCII label
// has no "xn--" prefix and is used as-is, so its punycode-encoded
// length is irrelevant. The ASCII check is a cheap short-circuit that
// avoids running the punycode encoder on labels that don't need it.
// ------------------------------------------------------------------
fn exceeds_max_a_label_length(value: &str) -> bool {
	pattern::RE_NON_ASCII.is_match(value) && utf16_len(&format_puny::encode(value)) + 4 > 63
}
// ------------------------------------------------------------------
// HasInvalidHyphens
//
// RFC 5891 §4.2.3.2: label must not start or end with a hyphen, and
// must not have hyphens in the 3rd and 4th codepoint positions (the
// ACE prefix reservation, e.g. "xn--"). Operates on `chars` (the
// per-codepoint string array) rather than raw string indices so
// supplementary-plane characters earlier in the label don't throw
// off the codepoint-position count.
// ------------------------------------------------------------------
fn has_invalid_hyphens(chars: &[String]) -> bool {
	if chars.first().is_some_and(|char| char == "-") || chars.last().is_some_and(|char| char == "-") {
		return true;
	}
	chars.get(2..).unwrap_or_default().concat().starts_with("--")
}
// ------------------------------------------------------------------
// IsUnicodeLabel
// ------------------------------------------------------------------
pub fn is_unicode_label(value: &str) -> bool {
	// RFC 5890 §2.3.2.1: Ensure Max Length
	if exceeds_max_a_label_length(value) {
		return false;
	}
	// RFC 5893 - Bidi Rule (applies whenever this label itself is RTL)
	if format_bidi::has_right_to_left_characters(value) && !format_bidi::satisfies_bidi_rule(value) {
		return false;
	}
	let chars: Vec<String> = value.chars().map(String::from).collect();
	let code_points: Vec<u32> = value.chars().map(u32::from).collect();
	let length = code_points.len();
	// RFC 5891 §4.2.3.2: Hyphen rules
	if has_invalid_hyphens(&chars) {
		return false;
	}
	// RFC 5891 §4.2.3.2 - Must not begin with a combining mark
	if chars
		.first()
		.is_some_and(|char| pattern::RE_COMBINING_MARK.is_match(char))
	{
		return false;
	}
	let mut has_japanese = false;
	// let hasArabicIndic = false // (no-spec-coverage)
	// let hasExtendedArabicIndic = false // (no-spec-coverage)
	for i in 0..length {
		let code_point = code_points[i];
		let char = &chars[i];
		// 1. DISALLOWED exceptions
		if pattern::RE_RFC5892_DISALLOWED.is_match(char) {
			return false;
		}
		// 1b. General category (PVALID) enforcement
		if !pattern::RE_PERMITTED_CATEGORY.is_match(char) {
			return false;
		}
		// 2. Collect Flags
		if pattern::RE_SCRIPT_JAPANESE.is_match(char) {
			has_japanese = true;
		}
		// if (Pattern.RE_ARABIC_INDIC_DIGIT.test(char)) hasArabicIndic = true  // (no-spec-coverage)
		// if (Pattern.RE_EXT_ARABIC_INDIC_DIGIT.test(char)) hasExtendedArabicIndic = true  // (no-spec-coverage)
		// 3. CONTEXTO / CONTEXTJ Neighbor Rules
		let prev = i
			.checked_sub(1)
			.and_then(|index| code_points.get(index))
			.copied()
			.filter(|&prev| prev != 0);
		let next = code_points.get(i + 1).copied().filter(|&next| next != 0);
		match code_point {
			0x00b7 => {
				if prev != Some(0x006c) || next != Some(0x006c) {
					return false;
				}
			} // MIDDLE DOT (Catalan)
			0x0375 => {
				if next.is_none() || !pattern::RE_SCRIPT_GREEK.is_match(&chars[i + 1]) {
					return false;
				}
			} // Greek KERAIA
			0x05f3 | 0x05f4 => {
				if prev.is_none() || !pattern::RE_SCRIPT_HEBREW.is_match(&chars[i - 1]) {
					return false;
				}
			} // Hebrew GERESH
			0x200c => {
				// ZWNJ - RFC 5892 Appendix A.1
				// Note: Full validation requires a Unicode joining type table. We reject
				// only when preceded by ASCII (U+0000-U+007F), which can never satisfy
				// the Virama or cursive-joining rules.
				if prev.is_none_or(|prev| prev < 0x0080 && !pattern::RE_VIRAMA.is_match(&chars[i - 1])) {
					return false;
				}
			}
			0x200d => {
				// ZWJ: RFC 5892 Appendix A.2
				if prev.is_none() || !pattern::RE_VIRAMA.is_match(&chars[i - 1]) {
					return false;
				}
			}
			0x30fb => {} // KATAKANA MIDDLE DOT, checked at end via hasJapanese
			_ => {}
		}
	}
	// 4. Global Context Validations (Post-loop)
	// RFC 5892 Appendix A.7 - Katakana Middle Dot requirement
	if value.contains('\u{30fb}') && !has_japanese {
		return false;
	}
	// RFC 5892 Appendix A.8/A.9 - Mixing Arabic Digits
	// if (hasArabicIndic && hasExtendedArabicIndic) return false // (no-spec-coverage)
	true
}
