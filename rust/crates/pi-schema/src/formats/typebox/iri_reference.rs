//! Port of `packages/schema/src/formats/typebox/iri_reference.ts`.
//! PORT-STATUS: ported
//!
//! `URL.canParse` is the `url` crate's WHATWG parser; it can differ from V8's where rust-url lags the URL Standard.
//! Known: rust-url rejects hosts V8 accepts, namely invalid `xn--` labels (`http://xn--a/`) and hosts failing the IDNA
//! bidi rule.
//! rust-url rejects a reference with three or more leading slashes, which the URL Standard reads against the `http`
//! base as `//` (its special authority ignore slashes state), so the reference is passed with two.

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

// Rejects unencoded spaces (0x20), backslashes, control characters (0x00-0x1F, 0x7F), or malformed percent-encodings
static INVALID_IRI_CHARS: LazyLock<fancy_regex::Regex> = LazyLock::new(|| {
	fancy_regex::Regex::new(r"[\x00-\x20\x7F\\]|%(?![0-9a-fA-F]{2})").expect("INVALID_IRI_CHARS is a valid pattern")
});
// Detects malformed scheme/authority patterns lacking a colon (e.g. "httpx//example.com")
static MALFORMED_SCHEME: LazyLock<Regex> =
	LazyLock::new(|| Regex::new(r"^[a-zA-Z][a-zA-Z0-9+\-.]*\/\/").expect("MALFORMED_SCHEME is a valid pattern"));

/// Returns true if the value is an IRI reference
/// @specification https://tools.ietf.org/html/rfc3987
pub fn is_iri_reference(value: &str) -> bool {
	!INVALID_IRI_CHARS.is_match(value).unwrap_or(false)
		&& !MALFORMED_SCHEME.is_match(value)
		&& ::url::Url::parse("http://example.com")
			.and_then(|base| match value.trim_start_matches('/') {
				rest if value.len() - rest.len() > 2 => base.join(&format!("//{rest}")),
				_ => base.join(value),
			})
			.is_ok()
}
