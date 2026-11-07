//! Port of `packages/schema/src/formats/typebox/idna/format/puny.ts`.
//! PORT-STATUS: ported
//!
//! The arithmetic stays in `f64` like the JavaScript numbers it ports. `Decode` walks UTF-16 code units as
//! `charCodeAt` does. `String.fromCodePoint` builds UTF-16, where two decoded surrogate code points in a row form
//! one astral character; the Rust port does the same, and a lone surrogate, which Rust cannot hold, becomes U+FFFD,
//! which every caller treats the same way (neither is a letter, mark, digit or RTL character).

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
use pi_host::js::{number_to_string, utf16_len};
use pi_host::{PiError, PiResult};

use super::super::pattern::pattern;

// ------------------------------------------------------------------
// PunyCode (RFC 3492)
// ------------------------------------------------------------------
const PUNYCODE_BASE: f64 = 36.0;
const PUNYCODE_TMIN: f64 = 1.0;
const PUNYCODE_TMAX: f64 = 26.0;
const PUNYCODE_SKEW: f64 = 38.0;
const PUNYCODE_DAMP: f64 = 700.0;
const PUNYCODE_INITIAL_BIAS: f64 = 72.0;
const PUNYCODE_INITIAL_N: f64 = 128.0;

// ------------------------------------------------------------------
// ThrowDontCare
// ------------------------------------------------------------------
fn throw_dont_care() -> PiError {
	PiError::new("null")
}
// ------------------------------------------------------------------
// IsAcePrefixed
// ------------------------------------------------------------------
pub fn is_ace_prefixed(value: &str) -> bool {
	value.to_lowercase().starts_with("xn--")
}
// ------------------------------------------------------------------
// Adapt
// ------------------------------------------------------------------
fn adapt(delta: f64, num_points: f64, first_time: bool) -> f64 {
	let int32 = if delta.is_finite() {
		delta.trunc().rem_euclid(4_294_967_296.0) as u32 as i32
	} else {
		0
	};
	let mut delta = if first_time {
		(delta / PUNYCODE_DAMP).floor()
	} else {
		f64::from(int32 >> 1)
	};
	delta += (delta / num_points).floor();
	let mut k = 0.0;
	while delta > f64::from(((PUNYCODE_BASE - PUNYCODE_TMIN) * PUNYCODE_TMAX) as i32 >> 1) {
		delta = (delta / (PUNYCODE_BASE - PUNYCODE_TMIN)).floor();
		k += PUNYCODE_BASE;
	}
	k + (((PUNYCODE_BASE - PUNYCODE_TMIN + 1.0) * delta) / (delta + PUNYCODE_SKEW)).floor()
}
// ------------------------------------------------------------------
// Decode
// ------------------------------------------------------------------
pub fn decode(value: &str) -> PiResult<String> {
	let value: Vec<u16> = value.encode_utf16().collect();
	let mut output: Vec<f64> = Vec::new();
	let mut n = PUNYCODE_INITIAL_N;
	let mut i = 0.0;
	let mut bias = PUNYCODE_INITIAL_BIAS;
	let delim_idx = value.iter().rposition(|&unit| unit == 0x2d);
	if let Some(delim_idx) = delim_idx
		&& delim_idx > 0
	{
		for &cp in &value[..delim_idx] {
			if cp >= 128 {
				return Err(throw_dont_care()); // throw new Error('Invalid punycode: non-basic before delimiter')
			}
			output.push(f64::from(cp));
		}
	}
	let mut in_idx = delim_idx.map_or(0, |delim_idx| delim_idx + 1);
	while in_idx < value.len() {
		let oldi = i;
		let mut w = 1.0;
		let mut k = PUNYCODE_BASE;
		loop {
			let Some(&ch) = value.get(in_idx) else {
				return Err(throw_dont_care()); // throw new Error('Invalid punycode: unexpected end of input')
			};
			in_idx += 1;
			let digit: f64 = if (0x61..=0x7a).contains(&ch) {
				f64::from(ch - 0x61) // a-z => 0-25
			} else if (0x30..=0x39).contains(&ch) {
				f64::from(ch - 0x30 + 26) // 0-9 => 26-35
			// } else if (0x41..=0x5a).contains(&ch) { f64::from(ch - 0x41) // A-Z => 0-25 // (no-spec-coverage)
			} else {
				return Err(throw_dont_care()); // throw new Error('Invalid punycode: bad digit character')
			};
			i += digit * w;
			let t = if k <= bias {
				PUNYCODE_TMIN
			} else if k >= bias + PUNYCODE_TMAX {
				PUNYCODE_TMAX
			} else {
				k - bias
			};
			if digit < t {
				break;
			}
			w *= PUNYCODE_BASE - t;
			k += PUNYCODE_BASE;
		}
		let out_len = (output.len() + 1) as f64;
		bias = adapt(i - oldi, out_len, oldi == 0.0);
		n += (i / out_len).floor();
		i %= out_len;
		output.insert(i as usize, n);
		i += 1.0;
	}
	let mut units = Vec::with_capacity(output.len());
	for code_point in output {
		if code_point.fract() != 0.0 || !(0.0..=1_114_111.0).contains(&code_point) {
			return Err(PiError::named(
				"RangeError",
				format!("Invalid code point {}", number_to_string(code_point)),
			));
		}
		match char::from_u32(code_point as u32) {
			Some(c) => units.extend_from_slice(c.encode_utf16(&mut [0; 2])),
			None => units.push(code_point as u16),
		}
	}
	Ok(String::from_utf16_lossy(&units))
}
// ------------------------------------------------------------------
// DigitToChar
// ------------------------------------------------------------------
fn digit_to_char(digit: f64) -> String {
	if digit < 26.0 {
		char::from(digit as u8 + 0x61).to_string()
	} else {
		char::from(digit as u8 - 26 + 0x30).to_string()
	}
}
// ------------------------------------------------------------------
// Encode
//
// Encodes a Unicode string into its Punycode payload (the part that
// follows the 'xn--' ACE prefix). Used to compute the true A-label
// length of a U-label, since the 63-octet label length limit (RFC
// 5891 §4.2.4) applies to the encoded form, not the code point count.
// ------------------------------------------------------------------
static RE_REPLACE_NON_ASCII: LazyLock<Regex> =
	LazyLock::new(|| Regex::new(pattern::RE_NON_ASCII.as_str()).expect("RE_REPLACE_NON_ASCII is a valid pattern"));
pub fn encode(input: &str) -> String {
	// Extract basic ASCII characters and set initial output array state
	let basic = RE_REPLACE_NON_ASCII.replace_all(input, "");
	let basic_length = utf16_len(&basic) as f64;
	let mut result: Vec<String> = if basic_length > 0.0 {
		vec![basic.into_owned(), "-".to_string()]
	} else {
		Vec::new()
	};
	// Extract unicode codepoints
	let code_points: Vec<f64> = input.chars().map(|char| f64::from(u32::from(char))).collect();
	let mut n = PUNYCODE_INITIAL_N;
	let mut delta = 0.0;
	let mut bias = PUNYCODE_INITIAL_BIAS;
	let mut handled_cp_count = basic_length;
	while handled_cp_count < code_points.len() as f64 {
		let mut m = f64::INFINITY;
		for &cp in &code_points {
			if cp >= n && cp < m {
				m = cp;
			}
		}
		delta += (m - n) * (handled_cp_count + 1.0);
		n = m;
		for &cp in &code_points {
			if cp < n {
				delta += 1.0;
			}
			if cp == n {
				let mut q = delta;
				let mut k = PUNYCODE_BASE;
				loop {
					let t = if k <= bias {
						PUNYCODE_TMIN
					} else if k >= bias + PUNYCODE_TMAX {
						PUNYCODE_TMAX
					} else {
						k - bias
					};
					if q < t {
						break;
					}
					let digit = t + ((q - t) % (PUNYCODE_BASE - t));
					result.push(digit_to_char(digit));
					q = ((q - t) / (PUNYCODE_BASE - t)).floor();
					k += PUNYCODE_BASE;
				}
				result.push(digit_to_char(q));
				bias = adapt(delta, handled_cp_count + 1.0, handled_cp_count == basic_length);
				delta = 0.0;
				handled_cp_count += 1.0;
			}
		}
		delta += 1.0;
		n += 1.0;
	}
	result.join("")
}
