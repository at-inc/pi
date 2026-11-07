//! Port of `packages/schema/src/formats/typebox/idna/hostname.ts`.
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

use super::label::ascii as label_ascii;
use super::label::puny as label_puny;

fn is_valid_label_length(value: &str) -> bool {
	utf16_len(value) > 0 && utf16_len(value) <= 63
}
fn is_label(value: &str) -> bool {
	is_valid_label_length(value) && (label_puny::is_puny_label(value) || label_ascii::is_ascii_label(value))
}
pub fn is_hostname(value: &str) -> bool {
	if utf16_len(value) == 0 || utf16_len(value) > 253 {
		return false;
	}
	if value.encode_utf16().last() == Some(46) {
		return false;
	}
	value.split('.').all(is_label)
}
