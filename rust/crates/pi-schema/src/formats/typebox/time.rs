//! Port of `packages/schema/src/formats/typebox/time.ts`.
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
use std::sync::LazyLock;

use ::regex::Regex;

static TIME: LazyLock<Regex> = LazyLock::new(|| {
	Regex::new(r"(?-u)^(\d\d):(\d\d):(\d\d)(?:\.\d+)?(?:([Zz])|([+-])(\d\d):(\d\d))?$")
		.expect("TIME is a valid pattern")
});

/// Returns true if the value is an ISO time string
/// @specification https://datatracker.ietf.org/doc/html/rfc3339
pub fn is_time(value: &str, strict_time_zone: Option<bool>) -> bool {
	let strict_time_zone = strict_time_zone.unwrap_or(true);
	let Some(matches) = TIME.captures(value) else {
		return false;
	};
	let group = |index: usize| matches.get(index).map(|m| m.as_str()).filter(|text| !text.is_empty());
	let number = |index: usize| group(index).map_or(f64::NAN, |text| text.parse::<f64>().unwrap_or(f64::NAN));
	// Require timezone offset or 'Z'/'z' when strictTimeZone is true
	if strict_time_zone && group(4).is_none() && group(5).is_none() {
		return false;
	}
	let hr = number(1);
	let min = number(2);
	let sec = number(3);
	if hr > 23.0 || min > 59.0 || sec > 60.0 {
		return false;
	}
	if group(5).is_some() {
		let tz_h = number(6);
		let tz_m = number(7);
		if tz_h > 23.0 || tz_m > 59.0 {
			return false;
		}
	}
	if sec < 60.0 {
		return true;
	}
	// Leap second handling: must normalize to 23:59:60 UTC (1439 total UTC minutes)
	let tz_sign = if group(5) == Some("-") { -1.0 } else { 1.0 };
	let tz_h = if group(6).is_some() { number(6) } else { 0.0 };
	let tz_m = if group(7).is_some() { number(7) } else { 0.0 };
	let total_utc_min = hr * 60.0 + min - tz_sign * (tz_h * 60.0 + tz_m);
	((total_utc_min % 1440.0) + 1440.0) % 1440.0 == 1439.0
}
