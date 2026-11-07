//! Port of `packages/ai/src/utils/diagnostics.ts`.
//! PORT-STATUS: ported
//!
//! TypeScript takes the thrown value as `unknown`; the Rust port takes the `PiError` that stands in for it. A
//! TS error's own fields (`statusCode`, `headers`, `payload`, `error`, `code`) are read from `PiError.data`, and
//! `error.status` from `PiError.status`. Rust errors carry no stack, so `stack` is always absent.

use pi_host::PiError;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::types::{AssistantMessage, JsonObject};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DiagnosticErrorInfo {
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub name: Option<String>,
	pub message: String,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub stack: Option<String>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub code: Option<DiagnosticErrorCode>,
}

/// `DiagnosticErrorInfo.code`: `string | number`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum DiagnosticErrorCode {
	String(String),
	Number(f64),
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AssistantMessageDiagnostic {
	#[serde(rename = "type")]
	pub r#type: String,
	pub timestamp: f64,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub error: Option<DiagnosticErrorInfo>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub details: Option<JsonObject>,
}

pub fn format_thrown_value(value: &PiError) -> String {
	if value.message.is_empty() {
		value.name.clone()
	} else {
		value.message.clone()
	}
}

pub fn extract_diagnostic_error(error: &PiError) -> DiagnosticErrorInfo {
	let code = error
		.data
		.as_ref()
		.and_then(|data| data.get("code"))
		.and_then(|code| match code {
			Value::String(code) => Some(DiagnosticErrorCode::String(code.clone())),
			Value::Number(code) => code.as_f64().map(DiagnosticErrorCode::Number),
			_ => None,
		});
	DiagnosticErrorInfo {
		name: if error.name.is_empty() {
			None
		} else {
			Some(error.name.clone())
		},
		message: if error.message.is_empty() {
			error.name.clone()
		} else {
			error.message.clone()
		},
		stack: None,
		code,
	}
}

pub fn create_assistant_message_diagnostic(
	r#type: &str,
	error: &PiError,
	details: Option<JsonObject>,
) -> AssistantMessageDiagnostic {
	AssistantMessageDiagnostic {
		r#type: r#type.to_string(),
		timestamp: pi_host::now_ms(),
		error: Some(extract_diagnostic_error(error)),
		details,
	}
}

/// Preserve provider metadata that is lost when SDK errors become display strings.
pub fn create_provider_error_diagnostic(error: &PiError) -> AssistantMessageDiagnostic {
	let source = error.data.as_ref().and_then(Value::as_object);
	let field = |name: &str| source.and_then(|source| source.get(name));
	let status = match (error.status, field("status")) {
		(Some(status), _) => Some(Value::from(status)),
		(None, Some(status)) if status.is_number() => Some(status.clone()),
		_ => field("statusCode").filter(|status| status.is_number()).cloned(),
	};
	let headers = field("headers")
		.filter(|headers| headers.is_object() || headers.is_array())
		.cloned();
	let payload = match field("payload") {
		Some(payload) if !payload.is_null() => Some(payload.clone()),
		_ => field("error")
			.filter(|error| error.is_object() || error.is_array())
			.cloned(),
	};
	let mut details = JsonObject::new();
	for (key, value) in [("status", status), ("headers", headers), ("payload", payload)] {
		let Some(value) = value else { continue };
		details.insert(key.to_string(), value);
	}
	create_assistant_message_diagnostic("provider_error", error, Some(details))
}

pub fn append_assistant_message_diagnostic(message: &mut AssistantMessage, diagnostic: AssistantMessageDiagnostic) {
	message.diagnostics.get_or_insert_with(Vec::new).push(diagnostic);
}
