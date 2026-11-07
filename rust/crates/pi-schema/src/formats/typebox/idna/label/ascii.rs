//! Port of `packages/schema/src/formats/typebox/idna/label/ascii.ts`.
//! PORT-STATUS: reviewed

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
use super::super::pattern::pattern;

// -------------------------------------------------------------------
// Validates an ASCII LDH label per RFC 5891 §4.2.3.1
// -------------------------------------------------------------------
pub fn is_ascii_label(value: &str) -> bool {
	pattern::RE_RULE_HYPHEN_PLACEMENT.is_match(value).unwrap_or(false)
		&& pattern::RE_RULE_NOT_RESERVED_ACE.is_match(value).unwrap_or(false)
		&& pattern::RE_ASCII_LDH.is_match(value)
}
