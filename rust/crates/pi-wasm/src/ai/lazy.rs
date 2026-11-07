//! Exports of `api/lazy`: `lazyStream` over a JS setup function and `lazyApi` over a JS module loader.

use std::rc::Rc;

use js_sys::{Function, Reflect};
use pi_ai::api::lazy::{LazyApiCapabilities, LazyStreamObserver};
use pi_ai::{AssistantMessage, AssistantMessageEvent, ProviderStreams};
use wasm_bindgen::JsCast;
use wasm_bindgen::prelude::*;

use super::models::{JsStreams, NativeStreams};
use crate::events::EventSink;

/// `lazyStream(model, setup, observer)`: `setup()` resolves with a JS event stream, forwarded to `sink`.
#[wasm_bindgen(js_name = aiLazyStream)]
pub fn lazy_stream(model: JsValue, setup: Function, observer: JsValue, sink: EventSink) -> Result<(), JsValue> {
	let parsed = super::model(&model)?;
	let stream = pi_ai::api::lazy::lazy_stream(
		&parsed,
		move || async move {
			let source = crate::callback::call(&setup, &JsValue::UNDEFINED, &[]).await?;
			Ok(crate::events::from_js(source, None))
		},
		observer.is_object().then(|| observer_from_js(&observer)),
	);
	crate::events::send(stream, sink);
	Ok(())
}

/// The Rust observer of a JS `{ onEvent?, onResult? }`; each callback gets the event or result as a new JS value.
fn observer_from_js(observer: &JsValue) -> LazyStreamObserver {
	let method = |name: &str| {
		let this = observer.clone();
		Reflect::get(observer, &name.into())
			.ok()
			.and_then(|value| value.dyn_into::<Function>().ok())
			.map(|function| (function, this))
	};
	LazyStreamObserver {
		on_event: method("onEvent").map(|(function, this)| {
			Rc::new(move |event: &AssistantMessageEvent| {
				if let Ok(event) = crate::json::to_js(event) {
					let _ = function.call1(&this, &event);
				}
			}) as Rc<dyn Fn(&AssistantMessageEvent)>
		}),
		on_result: method("onResult").map(|(function, this)| {
			Rc::new(move |result: Option<&AssistantMessage>| {
				let result = result.map_or(Ok(JsValue::UNDEFINED), crate::json::to_js);
				if let Ok(result) = result {
					let _ = function.call1(&this, &result);
				}
			}) as Rc<dyn Fn(Option<&AssistantMessage>)>
		}),
	}
}

/// `lazyApi(load, capabilities)`: `load()` resolves with a JS `ProviderStreams`, called as JS.
#[wasm_bindgen(js_name = aiLazyApi)]
pub fn lazy_api(load: Function, fetch_deferred: Option<bool>, cancel_deferred: Option<bool>) -> NativeStreams {
	let loader = Rc::new(move || {
		let load = load.clone();
		Box::pin(async move {
			let module = crate::callback::call(&load, &JsValue::UNDEFINED, &[]).await?;
			Ok(Rc::new(JsStreams::new(module)) as Rc<dyn ProviderStreams>)
		}) as pi_host::LocalBoxFuture<'static, _>
	});
	NativeStreams::new(pi_ai::api::lazy::lazy_api(
		loader,
		Some(LazyApiCapabilities {
			fetch_deferred,
			cancel_deferred,
		}),
	))
}
