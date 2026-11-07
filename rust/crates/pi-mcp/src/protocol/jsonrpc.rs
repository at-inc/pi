//! Port of `packages/mcp/src/protocol/jsonrpc.ts`.
//! PORT-STATUS: reviewed
//!
//! The TS error classes become `PiError` values with the class name in `name` and their fields in `data`;
//! each class is a unit struct with a constructor and accessors. `McpAbortError` keeps the TS name
//! `AbortError` and is told apart from other aborts by `data.class == "McpAbortError"`. A `PiError` with an
//! empty `name` stands for a thrown value that is not an `Error`; `String(value)` is its `message`.
//!
//! `JsonRpcMessage` is a typed enum, so the `isJsonRpc*` guards run on raw `serde_json::Value`s and
//! `parse_json_rpc_message` turns a checked value into the enum. `toError` is not ported: every Rust error is
//! already a `PiError`.

use std::hash::{Hash, Hasher};

use pi_host::{PiError, PiResult};
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::{Map, Value, json};

/// `JsonRpcId = string | number`.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(untagged)]
pub enum JsonRpcId {
	String(String),
	Number(f64),
}

impl JsonRpcId {
	/// `String(id)`.
	pub fn to_js_string(&self) -> String {
		match self {
			JsonRpcId::String(value) => value.clone(),
			JsonRpcId::Number(value) => pi_host::js::number_to_string(*value),
		}
	}

	/// The id in a JSON value, if it is one.
	pub fn from_value(value: &Value) -> Option<JsonRpcId> {
		if !is_json_rpc_id(value) {
			return None;
		}
		match value {
			Value::String(value) => Some(JsonRpcId::String(value.clone())),
			Value::Number(number) => number.as_f64().map(JsonRpcId::Number),
			_ => None,
		}
	}
}

/// `Map` key semantics (SameValueZero): `0` and `-0` are the same key.
impl PartialEq for JsonRpcId {
	fn eq(&self, other: &Self) -> bool {
		match (self, other) {
			(JsonRpcId::String(a), JsonRpcId::String(b)) => a == b,
			(JsonRpcId::Number(a), JsonRpcId::Number(b)) => a == b || (a.is_nan() && b.is_nan()),
			_ => false,
		}
	}
}

impl Eq for JsonRpcId {}

impl Hash for JsonRpcId {
	fn hash<H: Hasher>(&self, state: &mut H) {
		match self {
			JsonRpcId::String(value) => {
				0u8.hash(state);
				value.hash(state);
			}
			JsonRpcId::Number(value) => {
				1u8.hash(state);
				let normalized = if *value == 0.0 { 0.0 } else { *value };
				normalized.to_bits().hash(state);
			}
		}
	}
}

impl From<&str> for JsonRpcId {
	fn from(value: &str) -> Self {
		JsonRpcId::String(value.to_string())
	}
}

impl From<String> for JsonRpcId {
	fn from(value: String) -> Self {
		JsonRpcId::String(value)
	}
}

impl From<f64> for JsonRpcId {
	fn from(value: f64) -> Self {
		JsonRpcId::Number(value)
	}
}

/// The literal `"2.0"` of every message's `jsonrpc` field.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum JsonRpcVersion {
	#[default]
	#[serde(rename = "2.0")]
	V2_0,
}

/// Deserializes an optional field so that `null` stays `Some(Value::Null)`, as `"key" in object` sees it.
pub(crate) fn present<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Option<Value>, D::Error> {
	Value::deserialize(deserializer).map(Some)
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct JsonRpcRequest {
	pub jsonrpc: JsonRpcVersion,
	pub id: JsonRpcId,
	pub method: String,
	#[serde(default, skip_serializing_if = "Option::is_none", deserialize_with = "present")]
	pub params: Option<Value>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct JsonRpcNotification {
	pub jsonrpc: JsonRpcVersion,
	pub method: String,
	#[serde(default, skip_serializing_if = "Option::is_none", deserialize_with = "present")]
	pub params: Option<Value>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct JsonRpcErrorObject {
	pub code: f64,
	pub message: String,
	#[serde(default, skip_serializing_if = "Option::is_none", deserialize_with = "present")]
	pub data: Option<Value>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct JsonRpcSuccessResponse {
	pub jsonrpc: JsonRpcVersion,
	pub id: JsonRpcId,
	pub result: Value,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct JsonRpcErrorResponse {
	pub jsonrpc: JsonRpcVersion,
	pub id: JsonRpcId,
	pub error: JsonRpcErrorObject,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(untagged)]
pub enum JsonRpcResponse {
	Success(JsonRpcSuccessResponse),
	Error(JsonRpcErrorResponse),
}

impl JsonRpcResponse {
	/// `message.id`.
	pub fn id(&self) -> &JsonRpcId {
		match self {
			JsonRpcResponse::Success(response) => &response.id,
			JsonRpcResponse::Error(response) => &response.id,
		}
	}
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(untagged)]
pub enum JsonRpcMessage {
	Request(JsonRpcRequest),
	Notification(JsonRpcNotification),
	Response(JsonRpcResponse),
}

impl JsonRpcMessage {
	/// `{ jsonrpc: "2.0", id, method, ...(params === undefined ? {} : { params }) }`.
	pub fn request(id: impl Into<JsonRpcId>, method: impl Into<String>, params: Option<Value>) -> Self {
		JsonRpcMessage::Request(JsonRpcRequest {
			jsonrpc: JsonRpcVersion::V2_0,
			id: id.into(),
			method: method.into(),
			params,
		})
	}

	/// `{ jsonrpc: "2.0", method, ...(params === undefined ? {} : { params }) }`.
	pub fn notification(method: impl Into<String>, params: Option<Value>) -> Self {
		JsonRpcMessage::Notification(JsonRpcNotification {
			jsonrpc: JsonRpcVersion::V2_0,
			method: method.into(),
			params,
		})
	}

	/// `{ jsonrpc: "2.0", id, result }`.
	pub fn result(id: impl Into<JsonRpcId>, result: Value) -> Self {
		JsonRpcMessage::Response(JsonRpcResponse::Success(JsonRpcSuccessResponse {
			jsonrpc: JsonRpcVersion::V2_0,
			id: id.into(),
			result,
		}))
	}

	/// `{ jsonrpc: "2.0", id, error }`.
	pub fn error(id: impl Into<JsonRpcId>, error: JsonRpcErrorObject) -> Self {
		JsonRpcMessage::Response(JsonRpcResponse::Error(JsonRpcErrorResponse {
			jsonrpc: JsonRpcVersion::V2_0,
			id: id.into(),
			error,
		}))
	}

	/// `"method" in message ? message.method : undefined`.
	pub fn method(&self) -> Option<&str> {
		match self {
			JsonRpcMessage::Request(request) => Some(&request.method),
			JsonRpcMessage::Notification(notification) => Some(&notification.method),
			JsonRpcMessage::Response(_) => None,
		}
	}

	/// `"id" in message ? message.id : undefined`.
	pub fn id(&self) -> Option<&JsonRpcId> {
		match self {
			JsonRpcMessage::Request(request) => Some(&request.id),
			JsonRpcMessage::Notification(_) => None,
			JsonRpcMessage::Response(response) => Some(response.id()),
		}
	}

	/// The message as the JSON value `JSON.stringify` sees.
	pub fn to_value(&self) -> Value {
		serde_json::to_value(self).expect("JSON-RPC messages are always serializable")
	}
}

/// `JSON_RPC_ERROR_CODES`.
#[allow(non_snake_case)]
pub mod JSON_RPC_ERROR_CODES {
	pub const PARSE_ERROR: f64 = -32700.0;
	pub const INVALID_REQUEST: f64 = -32600.0;
	pub const METHOD_NOT_FOUND: f64 = -32601.0;
	pub const INVALID_PARAMS: f64 = -32602.0;
	pub const INTERNAL_ERROR: f64 = -32603.0;
}

pub(crate) fn error_class(error: &PiError) -> Option<&str> {
	error.data.as_ref()?.get("class")?.as_str()
}

/// `class McpError extends Error { code; data }`.
pub struct McpError;

impl McpError {
	pub const NAME: &'static str = "McpError";

	/// `new McpError(code, message, data)`.
	#[allow(clippy::new_ret_no_self)]
	pub fn new(code: f64, message: impl Into<String>, data: Option<Value>) -> PiError {
		let mut fields = Map::new();
		fields.insert("code".into(), json!(code));
		if let Some(data) = data {
			fields.insert("data".into(), data);
		}
		PiError::named(Self::NAME, message).with_data(Value::Object(fields))
	}

	/// `error instanceof McpError`.
	pub fn is(error: &PiError) -> bool {
		error.name == Self::NAME
	}

	/// `error.code`.
	pub fn code(error: &PiError) -> Option<f64> {
		error.data.as_ref()?.get("code")?.as_f64()
	}

	/// `error.data`.
	pub fn data(error: &PiError) -> Option<Value> {
		error.data.as_ref()?.get("data").cloned()
	}
}

/// `class McpConnectionClosedError extends Error`.
pub struct McpConnectionClosedError;

impl McpConnectionClosedError {
	pub const NAME: &'static str = "McpConnectionClosedError";

	/// `new McpConnectionClosedError()`.
	#[allow(clippy::new_ret_no_self)]
	pub fn new() -> PiError {
		Self::with_message("MCP connection closed")
	}

	/// `new McpConnectionClosedError(message)`.
	pub fn with_message(message: impl Into<String>) -> PiError {
		PiError::named(Self::NAME, message)
	}

	/// `error instanceof McpConnectionClosedError`.
	pub fn is(error: &PiError) -> bool {
		error.name == Self::NAME
	}
}

/// `class McpTimeoutError extends Error { timeoutMs }`.
pub struct McpTimeoutError;

impl McpTimeoutError {
	pub const NAME: &'static str = "McpTimeoutError";

	/// `new McpTimeoutError(timeoutMs)`.
	#[allow(clippy::new_ret_no_self)]
	pub fn new(timeout_ms: f64) -> PiError {
		PiError::named(
			Self::NAME,
			format!(
				"MCP request timed out after {}ms",
				pi_host::js::number_to_string(timeout_ms)
			),
		)
		.with_data(json!({ "timeoutMs": timeout_ms }))
	}

	/// `error instanceof McpTimeoutError`.
	pub fn is(error: &PiError) -> bool {
		error.name == Self::NAME
	}

	/// `error.timeoutMs`.
	pub fn timeout_ms(error: &PiError) -> Option<f64> {
		error.data.as_ref()?.get("timeoutMs")?.as_f64()
	}
}

/// `class McpAbortError extends Error`, whose `name` is `"AbortError"`.
pub struct McpAbortError;

impl McpAbortError {
	pub const NAME: &'static str = "AbortError";

	/// `new McpAbortError()`.
	#[allow(clippy::new_ret_no_self)]
	pub fn new() -> PiError {
		Self::with_message("MCP request aborted")
	}

	/// `new McpAbortError(message)`.
	pub fn with_message(message: impl Into<String>) -> PiError {
		PiError::named(Self::NAME, message).with_data(json!({ "class": "McpAbortError" }))
	}

	/// `error instanceof McpAbortError`.
	pub fn is(error: &PiError) -> bool {
		error.name == Self::NAME && error_class(error) == Some("McpAbortError")
	}
}

/// `String(error)` for a thrown value: `Error.prototype.toString` for errors, the message itself for a thrown
/// non-Error (empty `name`).
pub fn js_error_string(error: &PiError) -> String {
	if error.name.is_empty() {
		return error.message.clone();
	}
	if error.message.is_empty() {
		return error.name.clone();
	}
	format!("{}: {}", error.name, error.message)
}

/// `typeof value === "object" && value !== null && !Array.isArray(value)`.
pub fn is_object(value: &Value) -> bool {
	value.is_object()
}

/// `typeof value === "string" || (typeof value === "number" && Number.isFinite(value))`.
pub fn is_json_rpc_id(value: &Value) -> bool {
	match value {
		Value::String(_) => true,
		Value::Number(number) => number.as_f64().is_some_and(f64::is_finite),
		_ => false,
	}
}

fn is_version_2(object: &Map<String, Value>) -> bool {
	object.get("jsonrpc").and_then(Value::as_str) == Some("2.0")
}

pub fn is_json_rpc_request(message: &Value) -> bool {
	let Some(object) = message.as_object() else {
		return false;
	};
	is_version_2(object)
		&& object.get("id").is_some_and(is_json_rpc_id)
		&& object.get("method").is_some_and(Value::is_string)
}

pub fn is_json_rpc_notification(message: &Value) -> bool {
	let Some(object) = message.as_object() else {
		return false;
	};
	is_version_2(object) && !object.contains_key("id") && object.get("method").is_some_and(Value::is_string)
}

pub fn is_json_rpc_response(message: &Value) -> bool {
	let Some(object) = message.as_object() else {
		return false;
	};
	if !is_version_2(object) || !object.get("id").is_some_and(is_json_rpc_id) {
		return false;
	}
	if object.contains_key("result") {
		return !object.contains_key("error");
	}
	let Some(error) = object.get("error").and_then(Value::as_object) else {
		return false;
	};
	error.get("code").is_some_and(Value::is_number) && error.get("message").is_some_and(Value::is_string)
}

pub fn parse_json_rpc_message(value: Value) -> PiResult<JsonRpcMessage> {
	let is_request = is_json_rpc_request(&value);
	let is_notification = !is_request && is_json_rpc_notification(&value);
	let is_response = !is_request && !is_notification && is_json_rpc_response(&value);
	let Value::Object(mut object) = value else {
		return Err(invalid_message());
	};
	if !is_request && !is_notification && !is_response {
		return Err(invalid_message());
	}
	let id = object.get("id").and_then(JsonRpcId::from_value);
	if is_request || is_notification {
		let method = object
			.get("method")
			.and_then(Value::as_str)
			.expect("checked by the isJsonRpc guards")
			.to_string();
		let params = object.remove("params");
		return Ok(match id {
			Some(id) if is_request => JsonRpcMessage::request(id, method, params),
			_ => JsonRpcMessage::notification(method, params),
		});
	}
	let id = id.expect("checked by isJsonRpcResponse");
	if let Some(result) = object.remove("result") {
		return Ok(JsonRpcMessage::result(id, result));
	}
	let Some(Value::Object(mut error)) = object.remove("error") else {
		unreachable!("checked by isJsonRpcResponse");
	};
	let code = error
		.get("code")
		.and_then(Value::as_f64)
		.expect("checked by isJsonRpcResponse");
	let message = error
		.get("message")
		.and_then(Value::as_str)
		.expect("checked by isJsonRpcResponse")
		.to_string();
	Ok(JsonRpcMessage::error(
		id,
		JsonRpcErrorObject {
			code,
			message,
			data: error.remove("data"),
		},
	))
}

fn invalid_message() -> PiError {
	McpError::new(JSON_RPC_ERROR_CODES::INVALID_REQUEST, "Invalid JSON-RPC message", None)
}
