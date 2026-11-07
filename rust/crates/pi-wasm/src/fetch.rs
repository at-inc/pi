//! `fetch` across the boundary: a Rust `HttpRequest` becomes `fetch(url, init)` on a JS fetch function (the host's or
//! the caller's `options.fetch`), and the JS `Response` becomes an `HttpResponse` whose body streams the
//! `ReadableStream` chunk by chunk.

use bytes::Bytes;
use futures::StreamExt;
use indexmap::IndexMap;
use js_sys::{Array, Function, Object, Promise, Reflect, Uint8Array};
use pi_host::{HttpRequest, HttpResponse, LocalBoxFuture, PiResult, RedirectMode};
use wasm_bindgen::JsCast;
use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::JsFuture;

use crate::abort::JsSignal;

#[wasm_bindgen]
extern "C" {
	type ReadableStream;
	#[wasm_bindgen(method, catch, js_name = getReader)]
	fn get_reader(this: &ReadableStream) -> Result<StreamReader, JsValue>;

	type StreamReader;
	#[wasm_bindgen(method, catch)]
	fn read(this: &StreamReader) -> Result<Promise, JsValue>;
	#[wasm_bindgen(method, catch)]
	fn cancel(this: &StreamReader) -> Result<Promise, JsValue>;

	type ReadResult;
	#[wasm_bindgen(method, getter)]
	fn done(this: &ReadResult) -> bool;
	#[wasm_bindgen(method, getter)]
	fn value(this: &ReadResult) -> JsValue;
}

/// `fetch(request.url, init)` through `fetch`, called with `this`. `init` has `method`, `headers` as a plain object in
/// the request's order and casing, `body` as a string (or a `Uint8Array` when it is not UTF-8), `signal` following
/// the request's signal, and `redirect` unless it is the default `"follow"`. A rejection is the `PiError` of the
/// JS error, as the TS sees it.
pub fn js_fetch(
	fetch: Function,
	this: JsValue,
	request: HttpRequest,
) -> LocalBoxFuture<'static, PiResult<HttpResponse>> {
	Box::pin(async move {
		let init = Object::new();
		set(&init, "method", &request.method.as_str().into());
		let headers = Object::new();
		for (name, value) in &request.headers {
			set(&headers, name, &value.as_str().into());
		}
		set(&init, "headers", &headers);
		if let Some(body) = &request.body {
			let body = match std::str::from_utf8(body) {
				Ok(text) => JsValue::from_str(text),
				Err(_) => Uint8Array::from(&body[..]).into(),
			};
			set(&init, "body", &body);
		}
		let signal = request.signal.as_ref().map(crate::abort::to_js);
		if let Some(signal) = &signal {
			set(&init, "signal", &signal.js);
		}
		match request.redirect {
			RedirectMode::Follow => {}
			RedirectMode::Manual => set(&init, "redirect", &"manual".into()),
			RedirectMode::Error => set(&init, "redirect", &"error".into()),
		}
		let args = Array::of2(&request.url.as_str().into(), &init);
		let pending = fetch.apply(&this, &args).map_err(crate::error::from_js)?;
		let response = JsFuture::from(Promise::resolve(&pending))
			.await
			.map_err(crate::error::from_js)?;
		Ok(read_response(&response, signal))
	})
}

/// The `HttpResponse` of a JS `Response` (or a response-like object with the same fields).
pub fn read_response(response: &JsValue, signal: Option<JsSignal>) -> HttpResponse {
	let field = |name: &str| Reflect::get(response, &name.into()).unwrap_or(JsValue::UNDEFINED);
	let status = field("status").as_f64().unwrap_or(0.0) as u16;
	let status_text = field("statusText").as_string().unwrap_or_default();
	let url = field("url").as_string().unwrap_or_default();
	let headers = read_headers(&field("headers"));
	let body = readable_stream_body(&field("body"), signal);
	HttpResponse {
		status,
		status_text,
		url,
		headers,
		body,
	}
}

/// The `Body` of a JS `ReadableStream`, empty for `undefined` and `null`; `signal` lives as long as the body.
pub fn readable_stream_body(body: &JsValue, signal: Option<JsSignal>) -> pi_host::Body {
	if body.is_undefined() || body.is_null() {
		return futures::stream::empty().boxed_local();
	}
	match body.unchecked_ref::<ReadableStream>().get_reader() {
		Ok(reader) => body_stream(reader, signal),
		Err(error) => futures::stream::once(async move { Err(crate::error::from_js(error)) }).boxed_local(),
	}
}

/// Header entries, lowercased: a `Headers` iterates lowercased and combined already; a plain object is read in key
/// order.
fn read_headers(headers: &JsValue) -> IndexMap<String, String> {
	let mut out = IndexMap::new();
	if headers.is_undefined() || headers.is_null() {
		return out;
	}
	let entries: Vec<JsValue> = match js_sys::try_iter(headers) {
		Ok(Some(iter)) => iter.filter_map(Result::ok).collect(),
		_ => Object::entries(headers.unchecked_ref()).iter().collect(),
	};
	for entry in entries {
		let entry: Array = entry.unchecked_into();
		let (Some(name), Some(value)) = (entry.get(0).as_string(), entry.get(1).as_string()) else {
			continue;
		};
		out.insert(name.to_ascii_lowercase(), value);
	}
	out
}

/// Reads the body until `done`. Dropping the stream early cancels the reader, like abandoning a JS response body.
struct BodyReader {
	reader: StreamReader,
	finished: bool,
	_signal: Option<JsSignal>,
}

impl Drop for BodyReader {
	fn drop(&mut self) {
		if !self.finished {
			let _ = self.reader.cancel();
		}
	}
}

fn body_stream(reader: StreamReader, signal: Option<JsSignal>) -> pi_host::Body {
	let state = BodyReader {
		reader,
		finished: false,
		_signal: signal,
	};
	futures::stream::unfold(Some(state), |state| async move {
		let mut state = state?;
		let read = match state.reader.read() {
			Ok(promise) => JsFuture::from(promise).await,
			Err(error) => Err(error),
		};
		match read {
			Err(error) => {
				state.finished = true;
				Some((Err(crate::error::from_js(error)), None))
			}
			Ok(result) => {
				let result: ReadResult = result.unchecked_into();
				if result.done() {
					state.finished = true;
					return None;
				}
				let value = result.value();
				let chunk = match value.as_string() {
					Some(text) => Bytes::from(text.into_bytes()),
					None => Bytes::from(value.unchecked_into::<Uint8Array>().to_vec()),
				};
				Some((Ok(chunk), Some(state)))
			}
		}
	})
	.boxed_local()
}

fn set(target: &Object, key: &str, value: &JsValue) {
	let _ = Reflect::set(target, &key.into(), value);
}
