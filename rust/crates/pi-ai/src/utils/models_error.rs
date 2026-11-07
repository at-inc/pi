//! Port of `packages/ai/src/utils/models-error.ts`.
//! PORT-STATUS: reviewed
//!
//! `ModelsError` is a TS `Error` subclass. The port represents a thrown `ModelsError` as a `PiError` whose `name` is
//! `"ModelsError"` and whose `data` holds `{ code, cause? }`; `ModelsError::code_of` stands in for
//! `error instanceof ModelsError ? error.code : undefined`. The cause is kept as JSON: a `PiError` cause becomes
//! `{ name, message }` (plus `status` and `data` when present), any other thrown value is stored as it is.

use pi_host::PiError;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::utils::diagnostics::{Thrown, format_thrown_value};
use crate::utils::text::js_trim;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ModelsErrorCode {
	#[serde(rename = "model_source")]
	ModelSource,
	#[serde(rename = "model_validation")]
	ModelValidation,
	#[serde(rename = "provider")]
	Provider,
	#[serde(rename = "stream")]
	Stream,
	#[serde(rename = "auth")]
	Auth,
	#[serde(rename = "oauth")]
	Oauth,
}

impl ModelsErrorCode {
	/// The code's string value.
	pub fn as_str(self) -> &'static str {
		match self {
			Self::ModelSource => "model_source",
			Self::ModelValidation => "model_validation",
			Self::Provider => "provider",
			Self::Stream => "stream",
			Self::Auth => "auth",
			Self::Oauth => "oauth",
		}
	}
}

/// `class ModelsError extends Error`. `ModelsError::new` returns the thrown `PiError`.
pub struct ModelsError;

impl ModelsError {
	/// `new ModelsError(code, message, { cause })`.
	#[allow(clippy::new_ret_no_self, reason = "a thrown ModelsError is a PiError")]
	pub fn new<'a>(code: ModelsErrorCode, message: &str, cause: Option<Thrown<'a>>) -> PiError {
		let mut data = Map::new();
		data.insert("code".into(), Value::String(code.as_str().into()));
		if let Some(cause) = cause {
			data.insert("cause".into(), cause_value(cause));
		}
		PiError::named("ModelsError", with_cause_detail(message, cause)).with_data(Value::Object(data))
	}

	/// `error instanceof ModelsError ? error.code : undefined`.
	pub fn code_of(error: &PiError) -> Option<ModelsErrorCode> {
		if error.name != "ModelsError" {
			return None;
		}
		let code = error.data.as_ref()?.get("code")?;
		ModelsErrorCode::deserialize(code).ok()
	}

	/// `error instanceof ModelsError`.
	pub fn is(error: &PiError) -> bool {
		Self::code_of(error).is_some()
	}
}

fn cause_value(cause: Thrown<'_>) -> Value {
	match cause {
		Thrown::Value(value) => value.clone(),
		Thrown::Error(error) => {
			let mut object = Map::new();
			object.insert("name".into(), Value::String(error.name.clone()));
			object.insert("message".into(), Value::String(error.message.clone()));
			if let Some(status) = error.status {
				object.insert("status".into(), Value::from(status));
			}
			if let Some(data) = &error.data {
				object.insert("data".into(), data.clone());
			}
			Value::Object(object)
		}
	}
}

/// Callers surface `error.message` only, so keep the underlying reason in it.
fn with_cause_detail(message: &str, cause: Option<Thrown<'_>>) -> String {
	let Some(cause) = cause else {
		return message.to_string();
	};
	if matches!(cause, Thrown::Value(Value::Null)) {
		return message.to_string();
	}
	let detail = format_thrown_value(cause);
	let detail = js_trim(&detail);
	if detail.is_empty() || message.contains(detail) {
		return message.to_string();
	}
	format!("{message}: {detail}")
}
