//! Port of `packages/schema/src/formats/typebox/iri.ts`.
//! PORT-STATUS: ported
//!
//! `URL.canParse` is the `url` crate's WHATWG parser; it can differ from V8's where rust-url lags the URL Standard.
//! Known: rust-url rejects hosts V8 accepts, namely invalid `xn--` labels (`http://xn--a/`) and hosts failing the IDNA
//! bidi rule.
//! It accepts an empty host after `@` (`mailto://@`), which V8 rejects.

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
use std::borrow::Cow;
use std::sync::LazyLock;

use ::regex::Regex;
use pi_host::js::utf16_len;

const IPV_FUTURE_MATCH_MAX_LENGTH: usize = 2048;
static IPV_FUTURE_MATCH: LazyLock<Regex> =
	LazyLock::new(|| Regex::new(r"\[[vV][0-9a-fA-F]+\.[^\]]+\]").expect("IPV_FUTURE_MATCH is a valid pattern")); // Guarded By IpvFutureMatchMaxLength
static INVALID_IRI_CHARS: LazyLock<Regex> =
	LazyLock::new(|| Regex::new(r"[\x00-\x20<>^`{|}\\]").expect("INVALID_IRI_CHARS is a valid pattern"));
static INVALID_PERCENT_ENCODING: LazyLock<fancy_regex::Regex> = LazyLock::new(|| {
	fancy_regex::Regex::new(r"%(?![0-9a-fA-F]{2})").expect("INVALID_PERCENT_ENCODING is a valid pattern")
});

// ------------------------------------------------------------------
// NarrowIpvFuture
//
// Substitutes an IPvFuture address with a standard IPv6 loopback
// address ([::1]). We do this because the native URL.canParse
// (WHATWG standard) rejects IPvFuture literals, which are
// otherwise valid in RFC 3987. Because regex substitution can
// be expensive on large strings, this operation is strictly
// limited to inputs under a defined length threshold.
//
// (review-optimization)
//
// ------------------------------------------------------------------
fn narrow_ipv_future(value: &str) -> Cow<'_, str> {
	if utf16_len(value) < IPV_FUTURE_MATCH_MAX_LENGTH {
		IPV_FUTURE_MATCH.replace(value, "[::1]")
	} else {
		Cow::Borrowed(value)
	}
}
/// Returns true if the value is a valid Internationalized Resource Identifier.
/// @specification https://datatracker.ietf.org/doc/html/rfc3987
pub fn is_iri(value: &str) -> bool {
	// 1. Reject strings containing unencoded whitespace or illegal control characters.
	if INVALID_IRI_CHARS.is_match(value) {
		return false;
	}
	// 2. Reject malformed percent-encoding triplets.
	if INVALID_PERCENT_ENCODING.is_match(value).unwrap_or(false) {
		return false;
	}
	// 3. Delegate to the native URL parser, patching the IPvFuture edge case beforehand.
	::url::Url::parse(&narrow_ipv_future(value)).is_ok()
}
