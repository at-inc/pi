//! Exports of `protocol/jsonrpc`: the error codes and the message checks. The error classes are JS classes of the
//! facade.

use js_sys::{Object, Reflect};
use pi_mcp::JSON_RPC_ERROR_CODES;
use serde_json::Value;
use wasm_bindgen::prelude::*;

/// `JSON_RPC_ERROR_CODES`.
#[wasm_bindgen(js_name = mcpJsonRpcErrorCodes)]
pub fn json_rpc_error_codes() -> JsValue {
	let codes = Object::new();
	for (name, code) in [
		("parseError", JSON_RPC_ERROR_CODES::PARSE_ERROR),
		("invalidRequest", JSON_RPC_ERROR_CODES::INVALID_REQUEST),
		("methodNotFound", JSON_RPC_ERROR_CODES::METHOD_NOT_FOUND),
		("invalidParams", JSON_RPC_ERROR_CODES::INVALID_PARAMS),
		("internalError", JSON_RPC_ERROR_CODES::INTERNAL_ERROR),
	] {
		let _ = Reflect::set(&codes, &name.into(), &code.into());
	}
	codes.into()
}

/// `isJsonRpcRequest(message)`.
#[wasm_bindgen(js_name = mcpIsJsonRpcRequest)]
pub fn is_json_rpc_request(message: JsValue) -> bool {
	data(message).is_some_and(|message| pi_mcp::is_json_rpc_request(&message))
}

/// `isJsonRpcNotification(message)`.
#[wasm_bindgen(js_name = mcpIsJsonRpcNotification)]
pub fn is_json_rpc_notification(message: JsValue) -> bool {
	data(message).is_some_and(|message| pi_mcp::is_json_rpc_notification(&message))
}

/// `isJsonRpcResponse(message)`.
#[wasm_bindgen(js_name = mcpIsJsonRpcResponse)]
pub fn is_json_rpc_response(message: JsValue) -> bool {
	data(message).is_some_and(|message| pi_mcp::is_json_rpc_response(&message))
}

/// The check of `parseJsonRpcMessage(value)`: throws its `McpError` for a value that is no JSON-RPC message. The
/// facade returns the value itself, as the TS does.
#[wasm_bindgen(js_name = mcpParseJsonRpcMessage)]
pub fn parse_json_rpc_message(value: JsValue) -> Result<(), JsValue> {
	let value = crate::options::throw(crate::json::from_js::<Value>(value))?;
	crate::options::throw(pi_mcp::parse_json_rpc_message(value)).map(|_| ())
}

/// A JS value as JSON data; `None` for values that are not plain data, which no check accepts.
fn data(value: JsValue) -> Option<Value> {
	crate::json::from_js(value).ok()
}
