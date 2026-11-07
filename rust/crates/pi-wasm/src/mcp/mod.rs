//! Exports behind `@at-inc/pi-mcp-rs`, one file per facade module of `rust/npm/pi-mcp/src`. Export names are `mcp`
//! plus the TS name; handles are classes named after the TS object they stand for. Errors cross as JS `Error`s
//! with the Rust `name` and data fields; the facade gives them the prototype of its error class of that name.

mod auth_provider;
mod client;
mod content;
mod in_memory;
mod jsonrpc;
mod oauth_callback;
mod oauth_discovery;
mod oauth_flow;
mod oauth_provider;
mod stdio;
mod streamable_http;
mod transport;
mod types;

use std::future::Future;

use js_sys::{Function, Promise, Reflect};
use pi_host::{AbortSignal, PiError, PiResult};
use pi_mcp::McpFetch;
use pi_mcp::transports::transport::Unsubscribe;
use serde::Serialize;
use serde::de::DeserializeOwned;
use url::Url;
use wasm_bindgen::JsCast;
use wasm_bindgen::prelude::*;

#[wasm_bindgen]
extern "C" {
	/// `String(value)`.
	#[wasm_bindgen(js_name = String)]
	fn js_string(value: &JsValue) -> String;

	#[wasm_bindgen(js_name = URL)]
	type JsUrl;
	#[wasm_bindgen(constructor, catch, js_class = "URL")]
	fn new(url: &str) -> Result<JsUrl, JsValue>;
}

/// `object[key]`, `undefined` when absent or when `object` is not an object.
fn field(object: &JsValue, key: &str) -> JsValue {
	if !object.is_object() {
		return JsValue::UNDEFINED;
	}
	Reflect::get(object, &key.into()).unwrap_or(JsValue::UNDEFINED)
}

/// A function-valued field.
fn function(object: &JsValue, key: &str) -> Option<Function> {
	field(object, key).dyn_into::<Function>().ok()
}

/// An optional field read with serde: `undefined` and `null` are `None`.
fn optional<T: DeserializeOwned>(object: &JsValue, key: &str) -> Result<Option<T>, JsValue> {
	let value = field(object, key);
	if value.is_undefined() || value.is_null() {
		return Ok(None);
	}
	crate::options::arg(value).map(Some)
}

/// `String(value)` of a `string | URL` argument; `None` for `undefined` and `null`.
fn url_string(value: &JsValue) -> Option<String> {
	if value.is_undefined() || value.is_null() {
		return None;
	}
	Some(value.as_string().unwrap_or_else(|| js_string(value)))
}

/// A `string | URL` argument parsed like `new URL(value)`; `None` for `undefined` and `null`.
fn url_arg(value: &JsValue) -> Result<Option<Url>, JsValue> {
	let Some(text) = url_string(value) else {
		return Ok(None);
	};
	Url::parse(&text)
		.map(Some)
		.map_err(|_| crate::error::to_js(&PiError::type_error(format!("Invalid URL: {text}"))))
}

/// `url_arg` of a field.
fn url_field(object: &JsValue, key: &str) -> Result<Option<Url>, JsValue> {
	url_arg(&field(object, key))
}

/// `new URL(url)` as a JS value.
fn js_url(url: &str) -> PiResult<JsValue> {
	JsUrl::new(url).map(Into::into).map_err(crate::error::from_js)
}

/// `options.signal`.
fn signal(options: &JsValue) -> Option<AbortSignal> {
	crate::abort::from_js(&field(options, "signal"))
}

/// `options.fetch`.
fn fetch(options: &JsValue) -> Option<McpFetch> {
	function(options, "fetch").map(crate::options::fetch_function)
}

/// A JS function that calls `unsubscribe`; calling it again does nothing more than calling `unsubscribe` again.
fn unsubscribe_to_js(unsubscribe: Unsubscribe) -> JsValue {
	Closure::<dyn Fn()>::new(unsubscribe).into_js_value()
}

/// A Promise of a Rust result: the value as JS data, the error as a JS error.
fn promise<T: Serialize>(future: impl Future<Output = PiResult<T>> + 'static) -> Promise {
	wasm_bindgen_futures::future_to_promise(async move {
		let value = future.await.map_err(|error| crate::error::to_js(&error))?;
		crate::json::to_js(&value).map_err(|error| crate::error::to_js(&error))
	})
}

/// `promise` for an optional result, which resolves to `undefined` for `None`.
fn optional_promise<T: Serialize>(future: impl Future<Output = PiResult<Option<T>>> + 'static) -> Promise {
	wasm_bindgen_futures::future_to_promise(async move {
		match future.await.map_err(|error| crate::error::to_js(&error))? {
			Some(value) => crate::json::to_js(&value).map_err(|error| crate::error::to_js(&error)),
			None => Ok(JsValue::UNDEFINED),
		}
	})
}

/// `promise` for a result without a value, which resolves to `undefined`.
fn unit_promise(future: impl Future<Output = PiResult<()>> + 'static) -> Promise {
	wasm_bindgen_futures::future_to_promise(async move {
		future.await.map_err(|error| crate::error::to_js(&error))?;
		Ok(JsValue::UNDEFINED)
	})
}

/// A Promise from a call whose arguments may not fit: a bad argument rejects the Promise, as in an async TS function.
fn rejecting(call: impl FnOnce() -> Result<Promise, JsValue>) -> Promise {
	call().unwrap_or_else(|error| Promise::reject(&error))
}

/// `undefined` for `None`, the JS data of the value otherwise.
fn defined<T: Serialize>(value: Option<T>) -> Result<JsValue, JsValue> {
	match value {
		Some(value) => crate::options::throw(crate::json::to_js(&value)),
		None => Ok(JsValue::UNDEFINED),
	}
}

/// The value of a JS result that may be `undefined` or `null`.
fn optional_result<T: DeserializeOwned>(value: JsValue) -> PiResult<Option<T>> {
	if value.is_undefined() || value.is_null() {
		return Ok(None);
	}
	crate::json::from_js(value).map(Some)
}
