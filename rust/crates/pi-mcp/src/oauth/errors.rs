//! Port of `packages/mcp/src/oauth/errors.ts`.
//! PORT-STATUS: reviewed
//!
//! Each error class is a unit struct whose constructor returns a `PiError` with the class `name`; the class
//! fields live in `data` (camelCase, as in TS) and are read through accessors, like the protocol errors.

use pi_host::PiError;
use serde_json::json;

fn data_string(error: &PiError, key: &str) -> Option<String> {
	error.data.as_ref()?.get(key)?.as_str().map(str::to_string)
}

/// `class OAuthError extends Error { code; errorUri }`.
pub struct OAuthError;

impl OAuthError {
	pub const NAME: &'static str = "OAuthError";

	/// `new OAuthError(code, message, errorUri)`. An empty message falls back to the code.
	#[allow(clippy::new_ret_no_self)]
	pub fn new(code: &str, message: &str, error_uri: Option<&str>) -> PiError {
		let message = if message.is_empty() { code } else { message };
		PiError::named(Self::NAME, message).with_data(json!({ "code": code, "errorUri": error_uri }))
	}

	/// `error instanceof OAuthError`.
	pub fn is(error: &PiError) -> bool {
		error.name == Self::NAME
	}

	/// `error.code`.
	pub fn code(error: &PiError) -> Option<String> {
		data_string(error, "code")
	}

	/// `error.errorUri`.
	pub fn error_uri(error: &PiError) -> Option<String> {
		data_string(error, "errorUri")
	}
}

/// `class OAuthIssuerMismatchError extends Error { expected; received }`.
pub struct OAuthIssuerMismatchError;

impl OAuthIssuerMismatchError {
	pub const NAME: &'static str = "OAuthIssuerMismatchError";

	/// `new OAuthIssuerMismatchError(expected, received)`. `expected` is `None` when no authorization server
	/// metadata was discovered; `received` is `None` when an authorization response lacks the `iss` parameter
	/// its server promised (RFC 9207).
	#[allow(clippy::new_ret_no_self)]
	pub fn new(expected: Option<&str>, received: Option<&str>) -> PiError {
		let format = |issuer: Option<&str>| issuer.map_or_else(|| "none".to_string(), pi_host::js::json_stringify);
		PiError::named(
			Self::NAME,
			format!(
				"OAuth issuer mismatch: expected {}, received {}",
				format(expected),
				format(received)
			),
		)
		.with_data(json!({ "expected": expected, "received": received }))
	}

	/// `error instanceof OAuthIssuerMismatchError`.
	pub fn is(error: &PiError) -> bool {
		error.name == Self::NAME
	}

	/// `error.expected`.
	pub fn expected(error: &PiError) -> Option<String> {
		data_string(error, "expected")
	}

	/// `error.received`.
	pub fn received(error: &PiError) -> Option<String> {
		data_string(error, "received")
	}
}

/// `class OAuthInsecureEndpointError extends Error { endpoint }`.
pub struct OAuthInsecureEndpointError;

impl OAuthInsecureEndpointError {
	pub const NAME: &'static str = "OAuthInsecureEndpointError";

	/// `new OAuthInsecureEndpointError(endpoint)`.
	#[allow(clippy::new_ret_no_self)]
	pub fn new(endpoint: &str) -> PiError {
		PiError::named(
			Self::NAME,
			format!("Refusing to send OAuth credentials to non-HTTPS endpoint {endpoint}"),
		)
		.with_data(json!({ "endpoint": endpoint }))
	}

	/// `error instanceof OAuthInsecureEndpointError`.
	pub fn is(error: &PiError) -> bool {
		error.name == Self::NAME
	}

	/// `error.endpoint`.
	pub fn endpoint(error: &PiError) -> Option<String> {
		data_string(error, "endpoint")
	}
}

/// `class OAuthRegistrationError extends Error { status; body }`.
pub struct OAuthRegistrationError;

impl OAuthRegistrationError {
	pub const NAME: &'static str = "OAuthRegistrationError";

	/// `new OAuthRegistrationError(status, body)`.
	#[allow(clippy::new_ret_no_self)]
	pub fn new(status: u16, body: &str) -> PiError {
		PiError::named(
			Self::NAME,
			format!("OAuth dynamic client registration failed with status {status}: {body}"),
		)
		.with_status(status)
		.with_data(json!({ "status": status, "body": body }))
	}

	/// `error instanceof OAuthRegistrationError`.
	pub fn is(error: &PiError) -> bool {
		error.name == Self::NAME
	}

	/// `error.body`.
	pub fn body(error: &PiError) -> Option<String> {
		data_string(error, "body")
	}
}

/// `class McpOAuthAuthorizationRequiredError extends Error`.
pub struct McpOAuthAuthorizationRequiredError;

impl McpOAuthAuthorizationRequiredError {
	pub const NAME: &'static str = "McpOAuthAuthorizationRequiredError";

	/// `new McpOAuthAuthorizationRequiredError()`.
	#[allow(clippy::new_ret_no_self)]
	pub fn new() -> PiError {
		PiError::named(Self::NAME, "MCP OAuth authorization requires user interaction")
	}

	/// `error instanceof McpOAuthAuthorizationRequiredError`.
	pub fn is(error: &PiError) -> bool {
		error.name == Self::NAME
	}
}
