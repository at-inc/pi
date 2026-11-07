//! Option bags across the boundary. A TS options object mixes plain data with callbacks and an `AbortSignal`; `JsOptions`
//! splits it into the plain data (read with serde, key by key, as the Rust option structs need it) and the object
//! itself, from which the callbacks and the signal are read. Callbacks are called with the options object as `this`
//! and the caller's own JS model object, as the TS calls them.

use std::cell::RefCell;
use std::rc::{Rc, Weak};

use js_sys::{Function, Object, Reflect};
use pi_ai::{
	ClassifierModel, ClassifierOptions, DeferredCancelOptions, DeferredFetchOptions, ImageModel, ImagesOptions,
	OnPayload, OnProviderStreamEvent, OnResponse, ProviderImagesOptions, ProviderRequestOptions, ProviderResponse,
	ProviderStreamOptions, SimpleStreamOptions, StreamOptions, TransformHeadersFunction,
};
use pi_host::{FetchFunction, HttpRequest, HttpResponse, LocalBoxFuture, PiResult};
use serde::de::DeserializeOwned;
use serde_json::{Map, Value};
use wasm_bindgen::JsCast;
use wasm_bindgen::prelude::*;

use crate::abort::JsSignal;

/// Keys whose values are objects with identity, never plain data.
const OBJECT_KEYS: [&str; 2] = ["signal", "telemetry"];

/// A JS options object: its plain data fields and the object.
pub struct JsOptions {
	object: JsValue,
	data: Map<String, Value>,
}

impl JsOptions {
	/// Splits `object`; `undefined` and `null` give an empty bag. Functions, `undefined` values and the object-valued
	/// keys are not data; a value that is not plain data is skipped, as the Rust side could not read it anyway.
	pub fn new(object: JsValue) -> Self {
		let mut data = Map::new();
		if object.is_object() {
			for entry in Object::entries(object.unchecked_ref()) {
				let entry: js_sys::Array = entry.unchecked_into();
				let (Some(key), value) = (entry.get(0).as_string(), entry.get(1)) else {
					continue;
				};
				if value.is_undefined() || value.is_function() || OBJECT_KEYS.contains(&key.as_str()) {
					continue;
				}
				if let Ok(value) = crate::json::from_js::<Value>(value) {
					data.insert(key, value);
				}
			}
		}
		Self { object, data }
	}

	/// Whether the caller passed an options object.
	pub fn is_given(&self) -> bool {
		self.object.is_object()
	}

	/// Removes and reads a data field. A value the typed field cannot hold stays in the data and so reaches `extra`: the
	/// TS runtime does not validate option types, and an adapter that forwards the raw value (`toolChoice: "required"`
	/// in openai-completions) still sees it there.
	pub fn take<T: DeserializeOwned>(&mut self, key: &str) -> Option<T> {
		let value = serde_json::from_value(self.data.get(key)?.clone()).ok()?;
		self.data.shift_remove(key);
		Some(value)
	}

	/// A function-valued field.
	pub fn function(&self, key: &str) -> Option<Function> {
		Reflect::get(&self.object, &key.into())
			.ok()?
			.dyn_into::<Function>()
			.ok()
	}

	/// The remaining data fields, for `extra`.
	pub fn rest(self) -> Map<String, Value> {
		self.data
	}

	/// `ProviderRequestOptions`: `signal`, `apiKey`, `fetch`, `env`, `onPayload`, `onResponse`, `headers`,
	/// `identity`, `timeoutMs`, `maxRetries` and `maxRetryDelayMs`. `telemetry` is not bridged yet.
	pub fn request_options<M: 'static>(&mut self, model: &JsValue) -> ProviderRequestOptions<M> {
		let signal = Reflect::get(&self.object, &"signal".into()).unwrap_or(JsValue::UNDEFINED);
		ProviderRequestOptions {
			signal: crate::abort::from_js(&signal),
			telemetry: None,
			api_key: self.take("apiKey"),
			fetch: self.function("fetch").map(fetch_function),
			env: self.take("env"),
			on_payload: self
				.function("onPayload")
				.map(|f| on_payload(f, self.object.clone(), model.clone())),
			on_response: self
				.function("onResponse")
				.map(|f| on_response(f, self.object.clone(), model.clone())),
			headers: self.take("headers"),
			identity: self.take("identity"),
			timeout_ms: self.take("timeoutMs"),
			max_retries: self.take("maxRetries"),
			max_retry_delay_ms: self.take("maxRetryDelayMs"),
		}
	}

	/// `transformHeaders` of the `Models` request options, called with the assembled headers.
	pub fn transform_headers(&self) -> Option<TransformHeadersFunction> {
		let function = self.function("transformHeaders")?;
		let this = self.object.clone();
		Some(Rc::new(move |headers| {
			let (function, this) = (function.clone(), this.clone());
			Box::pin(async move {
				let headers = crate::json::to_js(&headers)?;
				crate::json::from_js(crate::callback::call(&function, &this, &[headers]).await?)
			})
		}))
	}

	/// `ProviderStreamOptions`: `StreamOptions` and the remaining fields as `extra`.
	pub fn provider_stream_options(mut self, model: &JsValue) -> ProviderStreamOptions {
		let base = self.stream_options(model);
		ProviderStreamOptions {
			base,
			extra: self.rest(),
		}
	}

	/// `SimpleStreamOptions`: `StreamOptions`, the simple fields and the remaining fields as `extra`.
	pub fn simple_stream_options(mut self, model: &JsValue) -> SimpleStreamOptions {
		let base = self.stream_options(model);
		SimpleStreamOptions {
			base,
			tool_choice: self.take("toolChoice"),
			reasoning: self.take("reasoning"),
			deferred: self.take("deferred"),
			thinking_budgets: self.take("thinkingBudgets"),
			extra: self.rest(),
		}
	}

	/// `ProviderImagesOptions`: `ImagesOptions` and the remaining fields as `extra`.
	pub fn images_options(mut self, model: &JsValue) -> ProviderImagesOptions {
		let base = ImagesOptions {
			base: self.request_options::<ImageModel>(model),
			metadata: self.take("metadata"),
		};
		ProviderImagesOptions {
			base,
			extra: self.rest(),
		}
	}

	/// `ClassifierOptions`.
	pub fn classifier_options(mut self, model: &JsValue) -> ClassifierOptions {
		ClassifierOptions {
			base: self.request_options::<ClassifierModel>(model),
			temperature: self.take("temperature"),
		}
	}

	/// `DeferredFetchOptions`.
	pub fn deferred_fetch_options(mut self, model: &JsValue) -> DeferredFetchOptions {
		DeferredFetchOptions {
			base: self.request_options(model),
			wait: self.take("wait"),
		}
	}

	/// `DeferredCancelOptions`.
	pub fn deferred_cancel_options(mut self, model: &JsValue) -> DeferredCancelOptions {
		self.request_options(model)
	}

	/// `StreamOptions`.
	pub fn stream_options(&mut self, model: &JsValue) -> StreamOptions {
		StreamOptions {
			base: self.request_options(model),
			on_provider_stream_event: self
				.function("onProviderStreamEvent")
				.map(|f| on_provider_stream_event(f, self.object.clone(), model.clone())),
			temperature: self.take("temperature"),
			sampling_params: self.take("samplingParams"),
			max_tokens: self.take("maxTokens"),
			transport: self.take("transport"),
			cache_retention: self.take("cacheRetention"),
			session_id: self.take("sessionId"),
			websocket_connect_timeout_ms: self.take("websocketConnectTimeoutMs"),
			metadata: self.take("metadata"),
		}
	}
}

/// `ProviderStreamOptions` of a JS options object; `None` when the caller passed none.
pub fn provider_stream_options(options: JsValue, model: &JsValue) -> Option<ProviderStreamOptions> {
	let options = JsOptions::new(options);
	options.is_given().then(|| options.provider_stream_options(model))
}

/// `SimpleStreamOptions` of a JS options object; `None` when the caller passed none.
pub fn simple_stream_options(options: JsValue, model: &JsValue) -> Option<SimpleStreamOptions> {
	let options = JsOptions::new(options);
	options.is_given().then(|| options.simple_stream_options(model))
}

thread_local! {
	/// The last `options.fetch` and its `FetchFunction`, both weak: while that `FetchFunction` lives, the same JS
	/// function maps to it again, as Rust code that keys a cache on the fetch's identity (the Anthropic federation
	/// client) compares the caller's JS function in the TS.
	static LAST_FETCH: RefCell<Option<(Weak<Function>, Weak<FetchClosure>)>> = const { RefCell::new(None) };
}

/// The closure behind a `FetchFunction`.
type FetchClosure = dyn Fn(HttpRequest) -> LocalBoxFuture<'static, PiResult<HttpResponse>>;

/// `options.fetch` as a `FetchFunction`, the same one for the same JS function while it is alive.
pub fn fetch_function(fetch: Function) -> FetchFunction {
	// `fetch: globalThis.fetch` is the host fetch itself, which adapters that refuse a custom fetch accept, as the TS
	// compares `options.fetch !== globalThis.fetch`.
	if Reflect::get(&js_sys::global(), &"fetch".into()).is_ok_and(|global| Object::is(&global, &fetch)) {
		return pi_host::global_fetch();
	}
	let cached = LAST_FETCH.with_borrow(|last| {
		let (function, fetch_function) = last.as_ref()?;
		let function = function.upgrade()?;
		Object::is(&function, &fetch).then(|| fetch_function.upgrade())?
	});
	if let Some(cached) = cached {
		return cached;
	}
	let function = Rc::new(fetch);
	let captured = function.clone();
	let fetch_function: FetchFunction =
		Rc::new(move |request| crate::fetch::js_fetch((*captured).clone(), JsValue::UNDEFINED, request));
	LAST_FETCH.set(Some((Rc::downgrade(&function), Rc::downgrade(&fetch_function))));
	fetch_function
}

/// The JS function of a `FetchFunction`: `globalThis.fetch` for the host fetch, the caller's function for the last
/// `options.fetch` while it lives. `None` for a fetch written in Rust, which has no JS function.
fn fetch_to_js(fetch: &FetchFunction) -> Option<JsValue> {
	if pi_host::is_global_fetch(fetch) {
		return Reflect::get(&js_sys::global(), &"fetch".into()).ok();
	}
	LAST_FETCH.with_borrow(|last| {
		let (function, fetch_function) = last.as_ref()?;
		let same = std::ptr::addr_eq(fetch_function.as_ptr(), Rc::as_ptr(fetch));
		same.then(|| function.upgrade())
			.flatten()
			.map(|function| (*function).clone().into())
	})
}

/// `options.onPayload(payload, model)`. A callback that returns `undefined` keeps the payload, including changes it made
/// to the payload object in place, as the TS sends the object it passed.
fn on_payload<M: 'static>(function: Function, this: JsValue, model: JsValue) -> OnPayload<M> {
	Rc::new(move |payload, _| {
		let (function, this, model) = (function.clone(), this.clone(), model.clone());
		Box::pin(async move {
			let payload = crate::json::to_js(&payload)?;
			let result = crate::callback::call(&function, &this, &[payload.clone(), model]).await?;
			let replacement = if result.is_undefined() { payload } else { result };
			Ok(Some(crate::json::from_js(replacement)?))
		})
	})
}

/// `options.onResponse({ status, headers }, model)`.
fn on_response<M: 'static>(function: Function, this: JsValue, model: JsValue) -> OnResponse<M> {
	Rc::new(move |response, _| {
		let (function, this, model) = (function.clone(), this.clone(), model.clone());
		Box::pin(async move {
			let object = Object::new();
			let _ = Reflect::set(&object, &"status".into(), &JsValue::from(response.status));
			let _ = Reflect::set(&object, &"headers".into(), &crate::json::to_js(&response.headers)?);
			crate::callback::call(&function, &this, &[object.into(), model]).await?;
			Ok(())
		})
	})
}

/// `options.onProviderStreamEvent(event, model)`.
pub fn on_provider_stream_event(function: Function, this: JsValue, model: JsValue) -> OnProviderStreamEvent {
	Rc::new(move |event, _| {
		let (function, this, model) = (function.clone(), this.clone(), model.clone());
		let event = crate::json::to_js(event);
		Box::pin(async move {
			crate::callback::call(&function, &this, &[event?, model]).await?;
			Ok(())
		})
	})
}

/// The JS options object for Rust options, for calling a JS implementation (`ProviderStreams` written in JS): the
/// plain fields, `signal` (alive while the returned guard lives), and `onPayload`, `onResponse` and
/// `onProviderStreamEvent` as JS functions over the Rust callbacks, and `fetch` as the caller's JS function (see
/// `fetch_to_js`). `telemetry` is not bridged this way yet.
pub fn stream_options_to_js(
	options: &StreamOptions,
	extra: &Map<String, Value>,
) -> PiResult<(JsValue, Option<JsSignal>)> {
	let mut fields = Map::new();
	let data: [(&str, Option<Value>); 8] = [
		("temperature", options.temperature.map(Value::from)),
		("samplingParams", options.sampling_params.clone().map(Value::Object)),
		("maxTokens", options.max_tokens.map(Value::from)),
		(
			"transport",
			serde_json::to_value(options.transport)
				.ok()
				.filter(|value| !value.is_null()),
		),
		(
			"cacheRetention",
			serde_json::to_value(options.cache_retention)
				.ok()
				.filter(|value| !value.is_null()),
		),
		("sessionId", options.session_id.clone().map(Value::from)),
		(
			"websocketConnectTimeoutMs",
			options.websocket_connect_timeout_ms.map(Value::from),
		),
		("metadata", options.metadata.clone().map(Value::Object)),
	];
	for (key, value) in data.into_iter().filter_map(|(key, value)| Some((key, value?))) {
		fields.insert(key.into(), value);
	}
	fields.extend(extra.iter().map(|(key, value)| (key.clone(), value.clone())));
	let (object, signal) = request_options_to_js(&options.base, &fields)?;
	if let Some(on_event) = options.on_provider_stream_event.clone() {
		let _ = Reflect::set(
			&object,
			&"onProviderStreamEvent".into(),
			&js_callback(move |args| {
				let on_event = on_event.clone();
				async move {
					let event: Value = crate::json::from_js(args.get(0))?;
					on_event(&event, &crate::json::from_js(args.get(1))?).await?;
					Ok(JsValue::UNDEFINED)
				}
			}),
		);
	}
	Ok((object, signal))
}

/// The JS options object of `ProviderRequestOptions` plus `extra` fields, as `stream_options_to_js` builds it, for the
/// JS implementations of every operation (streams, images, classifiers, deferred responses).
pub fn request_options_to_js<M: DeserializeOwned + 'static>(
	options: &ProviderRequestOptions<M>,
	extra: &Map<String, Value>,
) -> PiResult<(JsValue, Option<JsSignal>)> {
	let object = Object::new();
	let set = |key: &str, value: JsValue| {
		let _ = Reflect::set(&object, &key.into(), &value);
	};
	let signal = options.signal.as_ref().map(crate::abort::to_js);
	if let Some(signal) = &signal {
		set("signal", signal.js.clone());
	}
	if let Some(fetch) = options.fetch.as_ref().and_then(fetch_to_js) {
		set("fetch", fetch);
	}
	let data: [(&str, Option<Value>); 7] = [
		("apiKey", options.api_key.clone().map(Value::from)),
		(
			"env",
			serde_json::to_value(&options.env).ok().filter(|value| !value.is_null()),
		),
		(
			"headers",
			serde_json::to_value(&options.headers)
				.ok()
				.filter(|value| !value.is_null()),
		),
		(
			"identity",
			serde_json::to_value(&options.identity)
				.ok()
				.filter(|value| !value.is_null()),
		),
		("timeoutMs", options.timeout_ms.map(Value::from)),
		("maxRetries", options.max_retries.map(Value::from)),
		("maxRetryDelayMs", options.max_retry_delay_ms.map(Value::from)),
	];
	for (key, value) in data.into_iter().filter_map(|(key, value)| Some((key, value?))) {
		set(key, crate::json::to_js(&value)?);
	}
	for (key, value) in extra {
		set(key, crate::json::to_js(value)?);
	}
	if let Some(on_payload) = options.on_payload.clone() {
		set(
			"onPayload",
			js_callback(move |args| {
				let on_payload = on_payload.clone();
				async move {
					let payload = crate::json::from_js(args.get(0))?;
					let model: M = crate::json::from_js(args.get(1))?;
					let replaced = on_payload(payload, &model).await?;
					match replaced {
						Some(value) => crate::json::to_js(&value),
						None => Ok(JsValue::UNDEFINED),
					}
				}
			}),
		);
	}
	if let Some(on_response) = options.on_response.clone() {
		set(
			"onResponse",
			js_callback(move |args| {
				let on_response = on_response.clone();
				async move {
					let response = args.get(0);
					let status = Reflect::get(&response, &"status".into())
						.ok()
						.and_then(|status| status.as_f64());
					let headers = Reflect::get(&response, &"headers".into()).unwrap_or(JsValue::UNDEFINED);
					let response = ProviderResponse {
						status: status.unwrap_or(0.0) as u16,
						headers: crate::json::from_js(headers).unwrap_or_default(),
					};
					let model: M = crate::json::from_js(args.get(1))?;
					on_response(response, &model).await?;
					Ok(JsValue::UNDEFINED)
				}
			}),
		);
	}
	Ok((object.into(), signal))
}

/// `stream_options_to_js` for `SimpleStreamOptions`, adding `toolChoice`, `reasoning`, `deferred` and
/// `thinkingBudgets`.
pub fn simple_options_to_js(options: &SimpleStreamOptions) -> PiResult<(JsValue, Option<JsSignal>)> {
	let mut extra = options.extra.clone();
	let simple: [(&str, Value); 4] = [
		(
			"toolChoice",
			serde_json::to_value(options.tool_choice).unwrap_or_default(),
		),
		("reasoning", serde_json::to_value(options.reasoning).unwrap_or_default()),
		("deferred", serde_json::to_value(&options.deferred).unwrap_or_default()),
		(
			"thinkingBudgets",
			serde_json::to_value(&options.thinking_budgets).unwrap_or_default(),
		),
	];
	for (key, value) in simple {
		if !value.is_null() {
			extra.insert(key.into(), value);
		}
	}
	stream_options_to_js(&options.base, &extra)
}

/// A JS function over an async Rust callback: it returns a Promise of the callback's result, rejected with the JS
/// error of a `PiError`.
pub fn js_callback<F, Fut>(callback: F) -> JsValue
where
	F: Fn(js_sys::Array) -> Fut + 'static,
	Fut: std::future::Future<Output = PiResult<JsValue>> + 'static,
{
	Closure::<dyn Fn(JsValue, JsValue) -> js_sys::Promise>::new(move |first: JsValue, second: JsValue| {
		let pending = callback(js_sys::Array::of2(&first, &second));
		wasm_bindgen_futures::future_to_promise(
			async move { pending.await.map_err(|error| crate::error::to_js(&error)) },
		)
	})
	.into_js_value()
}

/// Reads a required argument with serde, as a thrown `TypeError` when it does not fit.
pub fn arg<T: DeserializeOwned>(value: JsValue) -> Result<T, JsValue> {
	crate::json::from_js(value).map_err(|error| crate::error::to_js(&error))
}

/// `PiResult` to a thrown JS error.
pub fn throw<T>(result: PiResult<T>) -> Result<T, JsValue> {
	result.map_err(|error| crate::error::to_js(&error))
}
