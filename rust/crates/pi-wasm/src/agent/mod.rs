//! Exports behind `@at-inc/pi-agent-core-rs`, one file per facade module of `rust/npm/pi-agent-core/src`. Export names
//! are `agent` plus the TS name; `AgentHandle` stands for the TS `Agent`. `bridge` turns JS tools, hooks and stream
//! functions into their Rust types and hook contexts into JS objects; `events` sends agent events to JS as changes.

mod agent;
mod agent_loop;
mod bridge;
mod events;
mod proxy;
mod stream_fn;

use js_sys::{Function, Object, Reflect};
use pi_host::PiResult;
use serde::de::DeserializeOwned;
use wasm_bindgen::{JsCast, JsValue};

/// `target[key]`, `undefined` when it cannot be read.
fn get(target: &JsValue, key: &str) -> JsValue {
	Reflect::get(target, &key.into()).unwrap_or(JsValue::UNDEFINED)
}

/// `target[key]` when it is a function.
fn function(target: &JsValue, key: &str) -> Option<Function> {
	get(target, key).dyn_into().ok()
}

/// `target[key] = value`.
fn set(target: &Object, key: &str, value: &JsValue) {
	let _ = Reflect::set(target, &key.into(), value);
}

/// A serde value from a JS value, `None` for `undefined` and `null`.
fn optional<T: DeserializeOwned>(value: JsValue) -> PiResult<Option<T>> {
	if value.is_undefined() || value.is_null() {
		return Ok(None);
	}
	crate::json::from_js(value).map(Some)
}
