//! Port of `packages/ai/src/utils/provider-retry.ts`.
//! PORT-STATUS: ported
//!
//! A TS `ProviderError` is an `Error` with own `status` and `headers` properties (the OpenAI and Anthropic SDK
//! `APIError` shape). The Rust port reads `status` from `PiError.status` and the headers from `PiError.data.headers`:
//! a `PiError` is a provider error when `data.headers` is present and is an object of header values or `null` (TS
//! `headers: undefined`). Header names are matched case-insensitively, like `Headers.get`. `Number.parseFloat` and
//! `Date.parse` are ported locally: `Date.parse` understands the IMF-fixdate form HTTP uses for `retry-after` (and the
//! RFC 2822 variants with a numeric zone), plus the ECMAScript date-time format with an explicit offset or a date only;
//! for other strings, which V8's lenient parser may still read, it returns NaN, so the delay falls back to exponential
//! backoff. The backoff sleep is `pi_host::sleep`; the request is any async closure.

use std::future::Future;

use pi_host::{AbortSignal, PiError, PiResult};
use serde_json::Value;

const DEFAULT_MAX_RETRY_DELAY_MS: f64 = 60_000.0;

#[derive(Clone, Debug, Default)]
pub struct ProviderRetryOptions {
	pub max_retries: Option<f64>,
	pub max_retry_delay_ms: Option<f64>,
	pub signal: Option<AbortSignal>,
	/// HTTP statuses that fail at once although the default policy would retry them.
	pub no_retry_statuses: Option<Vec<u16>>,
}

/// `ProviderError.headers`: `Some(None)` for a provider error without headers.
fn provider_error_headers(error: &PiError) -> Option<Option<&serde_json::Map<String, Value>>> {
	match error.data.as_ref()?.get("headers")? {
		Value::Object(headers) => Some(Some(headers)),
		Value::Null => Some(None),
		_ => None,
	}
}

fn is_provider_error(error: &PiError) -> bool {
	provider_error_headers(error).is_some()
}

/// `error.headers?.get(name)`.
fn header<'a>(error: &'a PiError, name: &str) -> Option<&'a str> {
	provider_error_headers(error)??
		.iter()
		.find(|(key, _)| key.eq_ignore_ascii_case(name))
		.and_then(|(_, value)| value.as_str())
}

/// Mirrors the pinned OpenAI/Anthropic SDK retry policy; review when either SDK is upgraded.
fn is_retryable_provider_error(error: &PiError) -> bool {
	let should_retry = header(error, "x-should-retry");
	if should_retry == Some("true") {
		return true;
	}
	if should_retry == Some("false") {
		return false;
	}

	let Some(status) = error.status else {
		return true;
	};
	status == 408 || status == 409 || status == 429 || status >= 500
}

fn validate_server_retry_delay_ms(
	delay_ms: f64,
	max_retry_delay_ms: Option<f64>,
	provider_error_message: &str,
) -> PiResult<f64> {
	let max_delay_ms = max_retry_delay_ms.unwrap_or(DEFAULT_MAX_RETRY_DELAY_MS);
	if max_delay_ms > 0.0 && delay_ms > max_delay_ms {
		return Err(PiError::new(format!(
			"Server requested {}s retry delay (max: {}s). {provider_error_message}",
			pi_host::js::number_to_string((delay_ms / 1000.0).ceil()),
			pi_host::js::number_to_string((max_delay_ms / 1000.0).ceil()),
		)));
	}
	Ok(delay_ms)
}

fn get_retry_delay_ms(error: &PiError, retry_index: f64, max_retry_delay_ms: Option<f64>) -> PiResult<f64> {
	if let Some(retry_after_ms) = header(error, "retry-after-ms").filter(|value| !value.is_empty()) {
		let value = parse_float(retry_after_ms);
		if value.is_finite() {
			return validate_server_retry_delay_ms(value, max_retry_delay_ms, &error.message);
		}
	}

	if let Some(retry_after) = header(error, "retry-after").filter(|value| !value.is_empty()) {
		let seconds = parse_float(retry_after);
		let delay_ms = if seconds.is_nan() {
			date_parse(retry_after) - pi_host::now_ms()
		} else {
			seconds * 1000.0
		};
		if delay_ms.is_finite() {
			return validate_server_retry_delay_ms(delay_ms, max_retry_delay_ms, &error.message);
		}
	}

	let exponential_delay = (0.5 * 2f64.powf(retry_index)).min(8.0) * 1000.0;
	Ok(exponential_delay * (1.0 - rand::random::<f64>() * 0.25))
}

fn create_abort_error() -> PiError {
	PiError::named("AbortError", "Request aborted")
}

async fn abortable_sleep(ms: f64, signal: Option<&AbortSignal>) -> PiResult<()> {
	if signal.is_some_and(AbortSignal::aborted) {
		return Err(create_abort_error());
	}
	pi_host::sleep(if ms.is_nan() { ms } else { ms.max(0.0) }, signal)
		.await
		.map_err(|_| create_abort_error())
}

/// Reproduce the retry behavior used by the OpenAI and Anthropic SDKs while making
/// their backoff sleep interruptible. Their built-in retry timers ignore the
/// request AbortSignal, so callers must invoke the SDK with `maxRetries: 0` and
/// wrap the request with this helper. Provider-requested delays above
/// `maxRetryDelayMs` fail immediately (60 seconds by default); set it to zero to
/// disable the limit.
pub async fn retry_provider_request<T, F, Fut>(mut request: F, options: ProviderRetryOptions) -> PiResult<T>
where
	F: FnMut() -> Fut,
	Fut: Future<Output = PiResult<T>>,
{
	let max_retries = options.max_retries.unwrap_or(0.0);
	let mut retries_remaining = max_retries;

	loop {
		// Each retry is a fresh SDK request, so X-Stainless-Retry-Count remains zero.
		let error = match request().await {
			Ok(value) => return Ok(value),
			Err(error) => error,
		};
		if options.signal.as_ref().is_some_and(AbortSignal::aborted) {
			return Err(create_abort_error());
		}
		if retries_remaining <= 0.0 || !is_provider_error(&error) || !is_retryable_provider_error(&error) {
			return Err(error);
		}
		if let Some(status) = error.status
			&& options
				.no_retry_statuses
				.as_ref()
				.is_some_and(|statuses| statuses.contains(&status))
		{
			return Err(error);
		}

		let retry_index = max_retries - retries_remaining;
		retries_remaining -= 1.0;
		let delay_ms = get_retry_delay_ms(&error, retry_index, options.max_retry_delay_ms)?;
		abortable_sleep(delay_ms, options.signal.as_ref()).await?;
	}
}

/// `StrWhiteSpaceChar`: JavaScript white space and line terminators.
fn is_js_space(ch: char) -> bool {
	(ch.is_whitespace() && ch != '\u{85}') || ch == '\u{feff}'
}

/// `Number.parseFloat(text)`: the longest decimal literal at the start of `text` after white space, else NaN.
fn parse_float(text: &str) -> f64 {
	let text = text.trim_start_matches(is_js_space);
	let bytes = text.as_bytes();
	let mut end = 0;
	if matches!(bytes.first(), Some(b'+' | b'-')) {
		end = 1;
	}
	if text[end..].starts_with("Infinity") {
		return if text.starts_with('-') {
			f64::NEG_INFINITY
		} else {
			f64::INFINITY
		};
	}
	let digits = |from: usize| from + bytes[from..].iter().take_while(|byte| byte.is_ascii_digit()).count();
	let integer_end = digits(end);
	let mut mantissa_end = integer_end;
	if bytes.get(integer_end) == Some(&b'.') {
		mantissa_end = digits(integer_end + 1);
		if mantissa_end == integer_end + 1 {
			mantissa_end = if integer_end > end {
				integer_end + 1
			} else {
				integer_end
			};
		}
	}
	if mantissa_end == end || (mantissa_end == end + 1 && bytes[end] == b'.') {
		return f64::NAN;
	}
	let mut literal_end = mantissa_end;
	if matches!(bytes.get(mantissa_end), Some(b'e' | b'E')) {
		let mut exponent_start = mantissa_end + 1;
		if matches!(bytes.get(exponent_start), Some(b'+' | b'-')) {
			exponent_start += 1;
		}
		let exponent_end = digits(exponent_start);
		if exponent_end > exponent_start {
			literal_end = exponent_end;
		}
	}
	text[..literal_end].trim_end_matches('.').parse().unwrap_or(f64::NAN)
}

const MONTHS: [&str; 12] = [
	"jan", "feb", "mar", "apr", "may", "jun", "jul", "aug", "sep", "oct", "nov", "dec",
];
const MS_PER_DAY: f64 = 86_400_000.0;

/// `Date.parse(text)` for the formats listed in the file header; NaN for any other string.
fn date_parse(text: &str) -> f64 {
	let text = text.trim_matches(is_js_space);
	parse_iso_date(text)
		.or_else(|| parse_http_date(text))
		.unwrap_or(f64::NAN)
}

/// `YYYY-MM-DD` (UTC) or `YYYY-MM-DDTHH:mm[:ss[.sss]]` with `Z` or `±HH:mm`.
fn parse_iso_date(text: &str) -> Option<f64> {
	let (date, time) = match text.split_once('T') {
		Some((date, time)) => (date, Some(time)),
		None => (text, None),
	};
	let mut parts = date.split('-');
	let year = fixed_number(parts.next()?, 4)?;
	let month = fixed_number(parts.next()?, 2)?;
	let day = fixed_number(parts.next()?, 2)?;
	if parts.next().is_some() || !(1.0..=12.0).contains(&month) || !(1.0..=31.0).contains(&day) {
		return None;
	}
	let Some(time) = time else {
		return Some(days_from_civil(year, month, day) * MS_PER_DAY);
	};
	let (clock, offset_ms) = if let Some(clock) = time.strip_suffix('Z') {
		(clock, 0.0)
	} else {
		let sign_index = time.rfind(['+', '-'])?;
		let (clock, offset) = time.split_at(sign_index);
		let (hours, minutes) = offset[1..].split_once(':')?;
		let offset_ms = (fixed_number(hours, 2)? * 60.0 + fixed_number(minutes, 2)?) * 60_000.0;
		(clock, if offset.starts_with('-') { -offset_ms } else { offset_ms })
	};
	let (clock, millis) = match clock.split_once('.') {
		Some((clock, fraction)) if !fraction.is_empty() && fraction.bytes().all(|byte| byte.is_ascii_digit()) => {
			(clock, fixed_number(&format!("{fraction:0<3}")[..3], 3)?)
		}
		Some(_) => return None,
		None => (clock, 0.0),
	};
	let mut fields = clock.split(':');
	let hour = fixed_number(fields.next()?, 2)?;
	let minute = fixed_number(fields.next()?, 2)?;
	let second = fields.next().map_or(Some(0.0), |second| fixed_number(second, 2))?;
	if fields.next().is_some() {
		return None;
	}
	Some(utc_ms(year, month, day, hour, minute, second)? + millis - offset_ms)
}

/// `[Wdy,] DD Mon YYYY HH:MM[:SS] zone` with `GMT`, `UTC`, `UT`, `Z` or `±HHMM`.
fn parse_http_date(text: &str) -> Option<f64> {
	let mut words: Vec<&str> = text.split([' ', ',']).filter(|word| !word.is_empty()).collect();
	if words
		.first()
		.is_some_and(|word| word.bytes().all(|byte| byte.is_ascii_alphabetic()) && word.len() >= 3)
		&& !MONTHS.contains(&words[0][..3].to_ascii_lowercase().as_str())
	{
		words.remove(0);
	}
	let [day, month, year, clock, zone] = words.as_slice() else {
		return None;
	};
	let day = digits_number(day)?;
	let month = MONTHS
		.iter()
		.position(|name| month.len() >= 3 && month[..3].eq_ignore_ascii_case(name))? as f64
		+ 1.0;
	let year = digits_number(year)?;
	let mut fields = clock.split(':');
	let hour = digits_number(fields.next()?)?;
	let minute = digits_number(fields.next()?)?;
	let second = fields.next().map_or(Some(0.0), digits_number)?;
	if fields.next().is_some() {
		return None;
	}
	let offset_ms = match zone.to_ascii_uppercase().as_str() {
		"GMT" | "UTC" | "UT" | "Z" => 0.0,
		zone if zone.len() == 5 && (zone.starts_with('+') || zone.starts_with('-')) => {
			let minutes = fixed_number(&zone[1..3], 2)? * 60.0 + fixed_number(&zone[3..], 2)?;
			if zone.starts_with('-') {
				-minutes * 60_000.0
			} else {
				minutes * 60_000.0
			}
		}
		_ => return None,
	};
	if !(1.0..=31.0).contains(&day) {
		return None;
	}
	Some(utc_ms(year, month, day, hour, minute, second)? - offset_ms)
}

fn fixed_number(text: &str, width: usize) -> Option<f64> {
	if text.len() != width {
		return None;
	}
	digits_number(text)
}

fn digits_number(text: &str) -> Option<f64> {
	if text.is_empty() || !text.bytes().all(|byte| byte.is_ascii_digit()) {
		return None;
	}
	text.parse().ok()
}

fn utc_ms(year: f64, month: f64, day: f64, hour: f64, minute: f64, second: f64) -> Option<f64> {
	if hour > 24.0 || minute > 59.0 || second > 59.0 || (hour == 24.0 && (minute > 0.0 || second > 0.0)) {
		return None;
	}
	Some(days_from_civil(year, month, day) * MS_PER_DAY + ((hour * 60.0 + minute) * 60.0 + second) * 1000.0)
}

/// Days since 1970-01-01 of a proleptic Gregorian date (Howard Hinnant's algorithm).
fn days_from_civil(year: f64, month: f64, day: f64) -> f64 {
	let year = if month <= 2.0 { year - 1.0 } else { year };
	let era = (year / 400.0).floor();
	let year_of_era = year - era * 400.0;
	let month_index = if month > 2.0 { month - 3.0 } else { month + 9.0 };
	let day_of_year = ((153.0 * month_index + 2.0) / 5.0).floor() + day - 1.0;
	let day_of_era = year_of_era * 365.0 + (year_of_era / 4.0).floor() - (year_of_era / 100.0).floor() + day_of_year;
	era * 146_097.0 + day_of_era - 719_468.0
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn parse_float_reads_the_leading_decimal_literal() {
		for (text, expected) in [
			("1000", 1000.0),
			("  12.5s", 12.5),
			("1e3", 1000.0),
			("1e", 1.0),
			(".5", 0.5),
			("5.", 5.0),
			("-2", -2.0),
			("+Infinity", f64::INFINITY),
			("0x10", 0.0),
		] {
			assert_eq!(parse_float(text), expected, "{text}");
		}
		for text in ["", ".", "abc", "-", "e5", "Wed, 21 Oct 2015 07:28:00 GMT"] {
			assert!(parse_float(text).is_nan(), "{text}");
		}
	}

	#[test]
	fn date_parse_reads_http_and_iso_dates() {
		assert_eq!(date_parse("Wed, 21 Oct 2015 07:28:00 GMT"), 1_445_412_480_000.0);
		assert_eq!(date_parse("21 Oct 2015 07:28:00 +0100"), 1_445_408_880_000.0);
		assert_eq!(date_parse("2015-10-21T07:28:00.5Z"), 1_445_412_480_500.0);
		assert_eq!(date_parse("2015-10-21T07:28:00+01:00"), 1_445_408_880_000.0);
		assert_eq!(date_parse("2015-10-21"), 1_445_385_600_000.0);
		assert_eq!(date_parse("2000-03-01"), 951_868_800_000.0);
		assert!(date_parse("soon").is_nan());
	}
}
