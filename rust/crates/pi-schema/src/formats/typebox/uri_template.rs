//! Port of `packages/schema/src/formats/typebox/uri_template.ts`.
//! PORT-STATUS: reviewed
//!
//! The pattern has the `i` flag without `u`, so it runs as an ASCII-only byte regex: JavaScript folds no
//! non-ASCII character to ASCII there (Unicode folding would let U+212A KELVIN SIGN match `k`), and the negated
//! class consumes a non-ASCII character byte by byte as JavaScript consumes it unit by unit.

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

static URI_TEMPLATE: LazyLock<Regex> = LazyLock::new(|| {
	Regex::new(r#"(?i-u)^(?:(?:[^\x00-\x20"<>%\\^`{|}\x7f]|%[0-9a-f]{2})|\{[+#./;?&=,!@|]?(?:[a-z0-9_]|%[0-9a-f]{2})+(?:\.(?:[a-z0-9_]|%[0-9a-f]{2})+)*(?::[1-9]\d{0,3}|\*)?(?:,(?:[a-z0-9_]|%[0-9a-f]{2})+(?:\.(?:[a-z0-9_]|%[0-9a-f]{2})+)*(?::[1-9]\d{0,3}|\*)?)*\})*$"#)
		.expect("URI_TEMPLATE is a valid pattern")
});

/// Returns true if the value is a valid Uri Template
/// @specification https://datatracker.ietf.org/doc/html/rfc6570
pub fn is_uri_template(value: &str) -> bool {
	URI_TEMPLATE.is_match(value.as_bytes())
}
