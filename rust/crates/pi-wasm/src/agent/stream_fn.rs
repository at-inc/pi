//! Exports of `stream-fn`.

use js_sys::Function;
use wasm_bindgen::JsCast;
use wasm_bindgen::prelude::*;

/// `setDefaultStreamFn(streamFn)`: the stream function of loops run without one; `undefined` clears it.
#[wasm_bindgen(js_name = agentSetDefaultStreamFn)]
pub fn set_default_stream_fn(stream_fn: JsValue) {
	pi_agent::set_default_stream_fn(
		stream_fn
			.dyn_into::<Function>()
			.ok()
			.map(|f| super::bridge::stream_fn(f, None)),
	);
}
