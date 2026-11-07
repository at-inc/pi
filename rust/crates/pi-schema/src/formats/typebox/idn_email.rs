//! Port of `packages/schema/src/formats/typebox/idn_email.ts`.
//! PORT-STATUS: ported
//!
//! The JavaScript `.` of the pattern (with `u`: any code point but a line terminator) is spelled
//! `[^\n\r\u{2028}\u{2029}]`; `\u{0080}-\u{10FFFF}` excludes the surrogates, which a Rust string cannot hold. A domain
//! label's `(?:[\p{L}\p{N}-]{0,62})(?<!-)` is spelled `(?:[\p{L}\p{N}-]{0,61}[\p{L}\p{N}])?`, the same labels without
//! a lookbehind, so the pattern runs on the `regex` crate, which has no backtracking limit to give up at on a long
//! value.

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
use unicode_normalization::UnicodeNormalization;

static IDN_EMAIL: LazyLock<Regex> = LazyLock::new(|| {
	Regex::new(r#"(?i)^(?:[A-Za-z0-9!#$%&'*+/=?^_`{|}~\x{0080}-\x{10FFFF}-]+(?:\.[A-Za-z0-9!#$%&'*+/=?^_`{|}~\x{0080}-\x{10FFFF}-]+)*|"(?:[^"\\]|\\[^\n\r\x{2028}\x{2029}])*")@[\p{L}\p{N}](?:[\p{L}\p{N}-]{0,61}[\p{L}\p{N}])?(?:\.[\p{L}\p{N}](?:[\p{L}\p{N}-]{0,61}[\p{L}\p{N}])?)*$"#)
		.expect("IDN_EMAIL is a valid pattern")
});

/// Returns true if the value is an IdnEmail
/// @specification https://datatracker.ietf.org/doc/html/rfc4952
pub fn is_idn_email(value: &str) -> bool {
	IDN_EMAIL.is_match(&value.nfc().collect::<String>())
}
