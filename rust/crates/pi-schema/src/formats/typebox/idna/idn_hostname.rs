//! Port of `packages/schema/src/formats/typebox/idna/idn-hostname.ts`.
//! PORT-STATUS: reviewed
//!
//! `normalize("NFC")` is `unicode-normalization`, on Unicode 18 data where Node 24 has Unicode 17.

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

use ::regex::{Captures, Regex};
use pi_host::js::utf16_len;
use unicode_normalization::UnicodeNormalization;

use super::format::bidi as format_bidi;
use super::label::puny as label_puny;
use super::label::unicode as label_unicode;

// ------------------------------------------------------------------
// IsLabel
// ------------------------------------------------------------------
fn is_valid_label_length(value: &str) -> bool {
	utf16_len(value) > 0 && utf16_len(value) <= 63
}
fn is_label(value: &str) -> bool {
	is_valid_label_length(value) && (label_puny::is_puny_label(value) || label_unicode::is_unicode_label(value))
}
// ------------------------------------------------------------------
// NormalizeHostname
//
// Normalizes a hostname for label validation. Maps fullwidth
// codepoints to their ASCII equivalent (e.g. 'ａ' U+FF41 maps to
// 'a'), then normalizes for NFC such that equivalent codepoint
// sequences can be uniformly compared. It also removes codepoints
// that IDNA mapping ignores, and converts full stop variants to
// ASCII '.' so the hostname can be split into labels.
// ------------------------------------------------------------------
static RE_FULLWIDTH_MAPPING: LazyLock<Regex> =
	LazyLock::new(|| Regex::new(r"[\x{ff01}-\x{ff5e}]").expect("RE_FULLWIDTH_MAPPING is a valid pattern"));
static RE_IGNORED_MAPPING: LazyLock<Regex> = LazyLock::new(|| {
	Regex::new(r"[\x{00ad}\x{034f}\x{180b}-\x{180d}\x{200b}\x{fe00}-\x{fe0f}\x{e0100}-\x{e01ef}]")
		.expect("RE_IGNORED_MAPPING is a valid pattern")
});
static RE_FULL_STOP_MAPPING: LazyLock<Regex> = LazyLock::new(|| {
	Regex::new(r"[\x{002E}\x{3002}\x{FF0E}\x{FF61}]").expect("RE_FULL_STOP_MAPPING is a valid pattern")
});
fn normalize_hostname(value: &str) -> String {
	let value = RE_FULLWIDTH_MAPPING.replace_all(value, |captures: &Captures| {
		captures[0]
			.chars()
			.map(|char| char::from((u32::from(char) - 0xfee0) as u8))
			.collect::<String>()
	});
	let value = value.nfc().collect::<String>();
	let value = RE_IGNORED_MAPPING.replace_all(&value, "");
	RE_FULL_STOP_MAPPING.replace_all(&value, ".").into_owned()
}
// ------------------------------------------------------------------
// IsIdnHostname
// ------------------------------------------------------------------
pub fn is_idn_hostname(value: &str) -> bool {
	if utf16_len(value) == 0 || value.contains(' ') {
		return false;
	}
	let normalized = normalize_hostname(value);
	if utf16_len(&normalized) > 253 {
		return false;
	}
	let labels: Vec<&str> = normalized.split('.').collect();
	let has_bidi_chars = labels.iter().any(|label| format_bidi::has_bidi_chars(label));
	labels
		.iter()
		.all(|label| is_label(label) && (!has_bidi_chars || format_bidi::satisfies_bidi_rule(label)))
}
