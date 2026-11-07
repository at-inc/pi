//! Port of `packages/schema/src/formats/typebox/uuid.ts`.
//! PORT-STATUS: reviewed
//!
//! The pattern has no `u` flag, so `(?-u)` keeps `\d` and the `i` flag ASCII-only, as in JavaScript (Unicode
//! case folding would let U+212A KELVIN SIGN match `k`).

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

static UUID: LazyLock<Regex> = LazyLock::new(|| {
	Regex::new(r"(?i-u)^[0-9a-f]{8}-(?:[0-9a-f]{4}-){3}[0-9a-f]{12}$").expect("UUID is a valid pattern")
});

/// Returns true if the value is a uuid
/// @specification https://www.rfc-editor.org/info/rfc4122/
/// @specification https://www.rfc-editor.org/info/rfc9562/
pub fn is_uuid(value: &str) -> bool {
	UUID.is_match(value)
}
