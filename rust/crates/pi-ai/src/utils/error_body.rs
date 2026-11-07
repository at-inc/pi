//! Port of `packages/ai/src/utils/error-body.ts`.
//! PORT-STATUS: reviewed
//!
//! Shared normalization for provider HTTP error objects.
//!
//! TypeScript probes the thrown value; the Rust port takes the `PiError` that stands in for it. The SDK fields
//! (`statusCode`, `status`, `body`, `error`, `$metadata`, `$response`) are read from `PiError.data`, and
//! `error.status` also from `PiError.status`. `PiError.data` is plain JSON, so every object in it is a plain
//! object and no value is a stream or a class instance. A thrown non-`Error` value has no Rust counterpart.

use pi_host::PiError;
use pi_host::js::{json_stringify, number_to_string, utf16_len};
use serde_json::Value;

use super::text::{js_trim, owned_slice};

pub const MAX_PROVIDER_ERROR_BODY_CHARS: usize = 4000;

#[derive(Clone, Debug, PartialEq)]
pub struct NormalizedProviderError {
	/// HTTP status code, when one could be extracted from the SDK error object.
	pub status: Option<f64>,
	/// Raw HTTP body reason, already trimmed and truncated to the cap.
	pub body: Option<String>,
	/// `error.message`.
	pub message: String,
	/// True when `message` already contains the body (no separate body to add).
	pub message_carries_body: bool,
}

pub fn normalize_provider_error(error: &PiError) -> NormalizedProviderError {
	let status = extract_status(error);
	let body = extract_body(error);
	let message_carries_body = body.as_ref().is_none_or(|body| error.message.contains(body.as_str()));

	NormalizedProviderError {
		status,
		body,
		message: error.message.clone(),
		message_carries_body,
	}
}

/// Probe the HTTP status, first numeric hit wins, in SDK-field order:
/// `statusCode` (Mistral) → `status` (`openai`, `@google/genai`) →
/// `$metadata.httpStatusCode` (Bedrock) → `$response.statusCode` (Bedrock).
fn extract_status(error: &PiError) -> Option<f64> {
	let field = |path: &[&str]| -> Option<f64> {
		let mut value = error.data.as_ref()?;
		for key in path {
			value = value.get(key)?;
		}
		value.as_f64()
	};
	field(&["statusCode"])
		.or_else(|| error.status.map(f64::from))
		.or_else(|| field(&["status"]))
		.or_else(|| field(&["$metadata", "httpStatusCode"]))
		.or_else(|| field(&["$response", "statusCode"]))
}

/// Probe the raw body reason, first usable hit wins, in SDK-field order:
/// `body` string (Mistral) → `error` parsed JSON body object (`openai` SDK's
/// `this.error`) → `$response.body` (Bedrock). Empty objects are treated as no body so they do not surface as
/// `"{}"`. The chosen body is truncated to the cap.
fn extract_body(error: &PiError) -> Option<String> {
	let body_text = pick_body_text(error.data.as_ref()?)?;
	let trimmed = js_trim(&body_text);
	if trimmed.is_empty() {
		return None;
	}
	Some(truncate_error_text(trimmed, MAX_PROVIDER_ERROR_BODY_CHARS))
}

fn pick_body_text(data: &Value) -> Option<String> {
	if let Some(Value::String(body)) = data.get("body") {
		return Some(body.clone());
	}
	if let Some(body) = data.get("error").filter(|value| is_non_empty_object(value)) {
		return Some(safe_json_stringify(body));
	}
	match data.get("$response").and_then(|response| response.get("body")) {
		Some(Value::String(body)) => Some(body.clone()),
		Some(body) if is_non_empty_object(body) => Some(safe_json_stringify(body)),
		_ => None,
	}
}

fn is_non_empty_object(value: &Value) -> bool {
	value.as_object().is_some_and(|object| !object.is_empty())
}

/// Compose a display string from a normalized error. When the message already
/// carries the body (Anthropic / `@google/genai` happy path) or no body/status
/// was extracted, the message is returned unchanged. Otherwise the status and
/// body are surfaced, with an optional provider prefix.
///
/// - no prefix: `"<status>: <body>"`
/// - prefix:    `"<prefix> (<status>): <body>"`
pub fn format_provider_error(norm: &NormalizedProviderError, prefix: Option<&str>) -> String {
	let (Some(status), Some(body), false) = (norm.status, norm.body.as_ref(), norm.message_carries_body) else {
		return match (prefix, norm.status) {
			(Some(prefix), Some(status)) => format!("{prefix} ({}): {}", number_to_string(status), norm.message),
			_ => norm.message.clone(),
		};
	};
	match prefix {
		Some(prefix) => format!("{prefix} ({}): {body}", number_to_string(status)),
		None => format!("{}: {body}", number_to_string(status)),
	}
}

/// Truncates `text` to `max_chars` UTF-16 code units and appends how many were cut.
pub fn truncate_error_text(text: &str, max_chars: usize) -> String {
	let length = utf16_len(text);
	if length <= max_chars {
		return text.to_string();
	}
	format!(
		"{}... [truncated {} chars]",
		owned_slice(text, 0, Some(max_chars)),
		length - max_chars
	)
}

/// `JSON.stringify(value)`. A JSON value always serializes, so the TS fallback to `String(value)` never applies.
pub fn safe_json_stringify(value: &Value) -> String {
	json_stringify(value)
}
