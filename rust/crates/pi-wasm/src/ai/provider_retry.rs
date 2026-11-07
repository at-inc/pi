//! Exports of `utils/provider-retry`. Each value `request` throws is read as the `PiError` the Rust loop takes (a
//! provider error carries `data.headers`, as the Rust port expects); when the loop gives up with one of them, the
//! export rethrows that JS value itself.

use std::cell::RefCell;
use std::rc::Rc;

use js_sys::{Function, Object, Promise, Reflect};
use pi_ai::utils::provider_retry::{self, ProviderRetryOptions};
use pi_host::PiError;
use serde::Deserialize;
use serde_json::Value;
use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::JsFuture;

use crate::json::from_js_text;
use crate::options::throw;

#[wasm_bindgen]
extern "C" {
	/// The global `Headers` class.
	#[wasm_bindgen(js_name = Headers)]
	type JsHeaders;
}

/// The `PiError.data` key that holds the index of the thrown JS value an error stands for.
const THROWN_INDEX: &str = "~piThrownIndex";

/// `retryProviderRequest(request, options)`.
#[wasm_bindgen(js_name = aiRetryProviderRequest)]
pub async fn retry_provider_request(request: Function, options: JsValue) -> Result<JsValue, JsValue> {
	let data: Options = throw(from_js_text(&options))?;
	let signal = if options.is_object() {
		crate::abort::from_js(&Reflect::get(&options, &"signal".into())?)
	} else {
		None
	};
	let options = ProviderRetryOptions {
		max_retries: data.max_retries,
		max_retry_delay_ms: data.max_retry_delay_ms,
		signal,
		no_retry_statuses: data.no_retry_statuses,
	};
	let thrown: Rc<RefCell<Vec<JsValue>>> = Rc::default();
	let attempts = thrown.clone();
	let result = provider_retry::retry_provider_request(
		move || {
			let request = request.clone();
			let thrown = attempts.clone();
			async move {
				let result = match request.call0(&JsValue::UNDEFINED) {
					Ok(result) => JsFuture::from(Promise::resolve(&result)).await,
					Err(error) => Err(error),
				};
				result.map_err(|error| {
					let mut thrown = thrown.borrow_mut();
					let converted = provider_error(&error, thrown.len());
					thrown.push(error);
					converted
				})
			}
		},
		options,
	)
	.await;
	result.map_err(|error| {
		let index = error
			.data
			.as_ref()
			.and_then(|data| data.get(THROWN_INDEX))
			.and_then(Value::as_u64);
		match index.and_then(|index| thrown.borrow().get(index as usize).cloned()) {
			Some(value) => value,
			None => crate::error::to_js(&error),
		}
	})
}

#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Options {
	max_retries: Option<f64>,
	max_retry_delay_ms: Option<f64>,
	no_retry_statuses: Option<Vec<u16>>,
}

/// The `PiError` of a thrown value. A TS `ProviderError` (an `Error` with own or inherited `status` and `headers`,
/// `status` a number or `undefined`, `headers` a `Headers` or `undefined`) gets its status and `data.headers`; any
/// other value gets no `data.headers`, so it is not retried.
fn provider_error(value: &JsValue, index: usize) -> PiError {
	let mut error = crate::error::from_js(value.clone());
	error.status = None;
	let mut data = serde_json::Map::new();
	data.insert(THROWN_INDEX.into(), Value::from(index));
	if value.is_instance_of::<js_sys::Error>()
		&& Reflect::has(value, &"status".into()).unwrap_or(false)
		&& Reflect::has(value, &"headers".into()).unwrap_or(false)
	{
		let status = Reflect::get(value, &"status".into()).unwrap_or(JsValue::UNDEFINED);
		let headers = Reflect::get(value, &"headers".into()).unwrap_or(JsValue::UNDEFINED);
		let status_ok = status.is_undefined() || status.as_f64().is_some();
		let headers_ok = headers.is_undefined() || headers.is_instance_of::<JsHeaders>();
		if status_ok && headers_ok {
			error.status = status.as_f64().map(|status| status as u16);
			let headers = if headers.is_undefined() {
				Value::Null
			} else {
				Object::from_entries(&headers)
					.ok()
					.and_then(|entries| from_js_text(&entries.into()).ok())
					.unwrap_or(Value::Null)
			};
			data.insert("headers".into(), headers);
		}
	}
	error.data = Some(Value::Object(data));
	error
}
