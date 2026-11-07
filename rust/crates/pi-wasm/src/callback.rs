//! JS callbacks: a JS function the caller passes is called like `await fn.call(this, ...args)`, so it may return a value
//! or a Promise, and a synchronous throw or a rejection becomes the `PiError` of the thrown value.

use js_sys::{Array, Function, Promise};
use pi_host::PiResult;
use wasm_bindgen::JsValue;
use wasm_bindgen_futures::JsFuture;

/// `await function.call(this, ...args)`.
pub async fn call(function: &Function, this: &JsValue, args: &[JsValue]) -> PiResult<JsValue> {
	let args: Array = args.iter().collect();
	let result = function.apply(this, &args).map_err(crate::error::from_js)?;
	JsFuture::from(Promise::resolve(&result))
		.await
		.map_err(crate::error::from_js)
}
