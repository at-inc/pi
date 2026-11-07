//! The error type that stands in for a thrown JavaScript `Error`.

use std::fmt;

/// A thrown JavaScript error. `message` must stay byte-identical to the TypeScript `Error.message`, because
/// messages flow into `AssistantMessage.errorMessage`, tool results, and test assertions.
#[derive(Clone, Debug, PartialEq)]
pub struct PiError {
	/// `Error.name` (`"Error"`, `"AbortError"`, `"TypeError"`, or a custom class name).
	pub name: String,
	/// `Error.message`.
	pub message: String,
	/// HTTP status for errors that came from a provider response (the SDK `APIError.status`).
	pub status: Option<u16>,
	/// Extra structured fields a TS error class carried (`code`, `cause`, response headers, ...).
	pub data: Option<serde_json::Value>,
}

/// `Result` whose error is a thrown JavaScript error.
pub type PiResult<T> = Result<T, PiError>;

impl PiError {
	/// `new Error(message)`.
	pub fn new(message: impl Into<String>) -> Self {
		Self {
			name: "Error".into(),
			message: message.into(),
			status: None,
			data: None,
		}
	}

	/// An error with a specific `name`, for TS subclasses of `Error`.
	pub fn named(name: impl Into<String>, message: impl Into<String>) -> Self {
		Self {
			name: name.into(),
			message: message.into(),
			status: None,
			data: None,
		}
	}

	/// `new TypeError(message)`.
	pub fn type_error(message: impl Into<String>) -> Self {
		Self::named("TypeError", message)
	}

	/// The `DOMException` an aborted `AbortSignal` throws by default.
	pub fn abort() -> Self {
		Self::named("AbortError", "This operation was aborted")
	}

	/// Attaches an HTTP status.
	pub fn with_status(mut self, status: u16) -> Self {
		self.status = Some(status);
		self
	}

	/// Attaches structured data.
	pub fn with_data(mut self, data: serde_json::Value) -> Self {
		self.data = Some(data);
		self
	}

	/// `error.name === "AbortError"`.
	pub fn is_abort(&self) -> bool {
		self.name == "AbortError"
	}
}

impl fmt::Display for PiError {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		f.write_str(&self.message)
	}
}

impl std::error::Error for PiError {}

impl From<serde_json::Error> for PiError {
	fn from(error: serde_json::Error) -> Self {
		Self::named("SyntaxError", error.to_string())
	}
}
