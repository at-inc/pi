//! Abort signals across the boundary, both ways: a JS `AbortSignal` the caller passes becomes a Rust `AbortSignal`
//! that aborts with the same reason, and a Rust signal becomes a JS `AbortSignal` for JS `fetch` and callbacks.
//!
//! One Rust signal is one JS signal for as long as it lives, as one JS object is in the TS: callbacks that compare the
//! signals they receive (`===`) see the same object, and a Rust signal made from a caller's JS signal crosses back as
//! that JS signal.

use std::cell::RefCell;
use std::collections::HashMap;

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
	remember(&rust, signal.clone(), |_| {});
	Some(rust)
}

thread_local! {
	/// The JS signal of each live Rust signal, by `AbortSignal::id`.
	static JS_SIGNALS: RefCell<HashMap<usize, JsValue>> = RefCell::new(HashMap::new());
}

/// Records `js` as the JS signal of `rust`. The entry is owned by a listener on the Rust signal, so it goes when the
/// signal aborts (after `on_abort` ran) or is dropped, and the id cannot be reused while the entry exists.
fn remember(rust: &AbortSignal, js: JsValue, on_abort: impl FnOnce(&pi_host::PiError) + 'static) {
	let entry = Entry(rust.id());
	JS_SIGNALS.with(|signals| signals.borrow_mut().insert(entry.0, js));
	rust.add_listener(move |reason| {
		on_abort(reason);
		drop(entry);
	});
}

struct Entry(usize);

impl Drop for Entry {
	fn drop(&mut self) {
		let _ = JS_SIGNALS.try_with(|signals| signals.borrow_mut().remove(&self.0));
	}
}

/// The JS `AbortSignal` of a Rust signal, for a JS call that takes one.
pub struct JsSignal {
	pub js: JsValue,
}

/// The JS `AbortSignal` of a Rust signal: the one it was made from or got before, else a new one that aborts with the
/// Rust signal's reason (already aborted when the Rust signal is).
pub fn to_js(signal: &AbortSignal) -> JsSignal {
	if let Some(js) = JS_SIGNALS.with(|signals| signals.borrow().get(&signal.id()).cloned()) {
		return JsSignal { js };
	}
	let controller = JsAbortController::new();
	let js = controller.signal();
	match signal.reason() {
		Some(reason) => controller.abort(&crate::error::to_js(&reason)),
		None => remember(signal, js.clone(), move |reason| controller.abort(&crate::error::to_js(reason))),
	}
	JsSignal { js }
}
