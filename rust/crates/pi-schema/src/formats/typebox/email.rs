//! Port of `packages/schema/src/formats/typebox/email.ts`.
//! PORT-STATUS: ported
//!
//! The pattern has the `i` flag without `u`, so it runs as an ASCII-only byte regex: JavaScript folds no
//! non-ASCII character to ASCII there (Unicode folding would let U+212A KELVIN SIGN match `k`), and the negated
//! class `[^"\\]` consumes a non-ASCII character byte by byte as JavaScript consumes it unit by unit.

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

use ::regex::bytes::Regex;

static EMAIL: LazyLock<Regex> = LazyLock::new(|| {
	Regex::new(r#"(?i-u)^(?:[a-z0-9!#$%&'*+/=?^_`{|}~-]+(?:\.[a-z0-9!#$%&'*+/=?^_`{|}~-]+)*|"(?:[^"\\]|\\[\x20-\x7e])*")@(?:[a-z0-9](?:[a-z0-9-]{0,61}[a-z0-9])?(?:\.[a-z0-9](?:[a-z0-9-]{0,61}[a-z0-9])?)*|\[(?:IPv6:[a-f0-9:]+|(?:25[0-5]|2[0-4][0-9]|1[0-9]{2}|[1-9]?[0-9])(?:\.(?:25[0-5]|2[0-4][0-9]|1[0-9]{2}|[1-9]?[0-9])){3})\])$"#)
		.expect("EMAIL is a valid pattern")
});

/// Returns true if the value is an Email
/// @specification https://datatracker.ietf.org/doc/html/rfc5322
pub fn is_email(value: &str) -> bool {
	EMAIL.is_match(value.as_bytes())
}
