//! Abort signals across the boundary, both ways: a JS `AbortSignal` the caller passes becomes a Rust `AbortSignal`
//! that aborts with the same reason, and a Rust signal becomes a JS `AbortSignal` for JS `fetch` and callbacks.

use pi_host::abort::ListenerId;
use pi_host::{AbortController, AbortSignal};
use wasm_bindgen::JsCast;
use wasm_bindgen::prelude::*;

#[wasm_bindgen]
extern "C" {
	#[wasm_bindgen(js_name = AbortController)]
	type JsAbortController;
	#[wasm_bindgen(constructor, js_class = "AbortController")]
	fn new() -> JsAbortController;
	#[wasm_bindgen(method, getter)]
	fn signal(this: &JsAbortController) -> JsValue;
	#[wasm_bindgen(method)]
	fn abort(this: &JsAbortController, reason: &JsValue);

	#[wasm_bindgen(js_name = AbortSignal)]
	type JsAbortSignal;
	#[wasm_bindgen(method, getter)]
	fn aborted(this: &JsAbortSignal) -> bool;
	#[wasm_bindgen(method, getter)]
	fn reason(this: &JsAbortSignal) -> JsValue;
	#[wasm_bindgen(method, js_name = addEventListener)]
	fn add_event_listener(this: &JsAbortSignal, kind: &str, listener: &JsValue, options: &JsValue);
}

/// The Rust signal for a JS `AbortSignal`, `None` for `undefined`/`null`. An aborted JS signal gives an aborted Rust
/// signal; otherwise the Rust signal aborts with the JS reason when the JS signal fires `abort`. The listener holds no
/// reference to the JS signal, so a never-aborted signal is collected with its listener.
pub fn from_js(signal: &JsValue) -> Option<AbortSignal> {
	if signal.is_undefined() || signal.is_null() {
		return None;
	}
	let js: &JsAbortSignal = signal.unchecked_ref();
	if js.aborted() {
		return Some(AbortSignal::aborted_with(crate::error::from_js(js.reason())));
	}
	let controller = AbortController::new();
	let rust = controller.signal();
	let mut controller = Some(controller);
	let listener = Closure::<dyn FnMut(JsValue)>::new(move |event: JsValue| {
		let Some(controller) = controller.take() else { return };
		let target = js_sys::Reflect::get(&event, &"target".into()).unwrap_or(JsValue::UNDEFINED);
		let reason = target.unchecked_ref::<JsAbortSignal>().reason();
		controller.abort_with(crate::error::from_js(reason));
	})
	.into_js_value();
	let options = js_sys::Object::new();
	let _ = js_sys::Reflect::set(&options, &"once".into(), &JsValue::TRUE);
	js.add_event_listener("abort", &listener, &options);
	Some(rust)
}

/// A JS `AbortSignal` that follows a Rust signal while this value lives. Dropping it removes the listener from the Rust
/// signal, so a long-lived signal does not collect one listener per request.
pub struct JsSignal {
	pub js: JsValue,
	source: AbortSignal,
	listener: Option<ListenerId>,
}

/// A JS `AbortSignal` that aborts with the Rust signal's reason, already aborted when the Rust signal is.
pub fn to_js(signal: &AbortSignal) -> JsSignal {
	let controller = JsAbortController::new();
	let js = controller.signal();
	let listener = match signal.reason() {
		Some(reason) => {
			controller.abort(&crate::error::to_js(&reason));
			None
		}
		None => Some(signal.add_listener(move |reason| controller.abort(&crate::error::to_js(reason)))),
	};
	JsSignal {
		js,
		source: signal.clone(),
		listener,
	}
}

impl Drop for JsSignal {
	fn drop(&mut self) {
		if let Some(listener) = self.listener {
			self.source.remove_listener(listener);
		}
	}
}
