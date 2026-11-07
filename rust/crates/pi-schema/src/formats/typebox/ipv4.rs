//! Port of `packages/schema/src/formats/typebox/ipv4.ts`.
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

static IPV4: LazyLock<Regex> = LazyLock::new(|| {
	Regex::new(r"(?-u)^(?:(?:25[0-5]|2[0-4]\d|1\d\d|[1-9]?\d)\.){3}(?:25[0-5]|2[0-4]\d|1\d\d|[1-9]?\d)$")
		.expect("IPV4 is a valid pattern")
});

/// Returns true if the value is a IPV4 address
/// @specification http://tools.ietf.org/html/rfc2673#section-3.2
pub fn is_ipv4(value: &str) -> bool {
	IPV4.is_match(value)
}
