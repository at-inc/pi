//! Port of `packages/schema/src/formats/typebox/date.ts`.
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

const DAYS: [f64; 13] = [
	0.0, 31.0, 28.0, 31.0, 30.0, 31.0, 30.0, 31.0, 31.0, 30.0, 31.0, 30.0, 31.0,
];
static DATE: LazyLock<Regex> =
	LazyLock::new(|| Regex::new(r"(?-u)^(\d\d\d\d)-(\d\d)-(\d\d)$").expect("DATE is a valid pattern"));

fn is_leap_year(year: f64) -> bool {
	year % 4.0 == 0.0 && (year % 100.0 != 0.0 || year % 400.0 == 0.0)
}
/// Returns true if the value is a ISO8601 Date component string
/// @description https://datatracker.ietf.org/doc/html/rfc3339
/// @example `2020-12-12`
pub fn is_date(value: &str) -> bool {
	let Some(matches) = DATE.captures(value) else {
		return false;
	};
	let number = |index: usize| {
		matches
			.get(index)
			.map_or(f64::NAN, |m| m.as_str().parse::<f64>().unwrap_or(f64::NAN))
	};
	let year: f64 = number(1);
	let month: f64 = number(2);
	let day: f64 = number(3);
	(1.0..=12.0).contains(&month)
		&& (1.0..=(if month == 2.0 && is_leap_year(year) {
			29.0
		} else {
			DAYS[month as usize]
		}))
			.contains(&day)
}
