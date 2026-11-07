//! Exports of `api/anthropic-messages` and `api/anthropic-messages.lazy`.

use std::rc::Rc;

use js_sys::{Function, Object, Promise, Reflect};
use pi_ai::api::anthropic_client::{AnthropicClientApi, RequestOptions};
use pi_ai::api::anthropic_messages::{self, AnthropicOptions};
use pi_ai::api::anthropic_messages_lazy;
use pi_host::{HttpResponse, LocalBoxFuture, PiResult};
use serde_json::Value;
use wasm_bindgen::JsCast;
use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::JsFuture;

use super::models::NativeStreams;
use crate::events::EventSink;
use crate::options::provider_stream_options;

/// ` stream(model, context, options)` of the module, sending the events to `sink`. `options.client`, a pre-built SDK
/// client such as `AnthropicVertex`, is called through JS (see `JsAnthropicClient`).
#[wasm_bindgen(js_name = aiAnthropicMessagesStream)]
pub fn stream(model: JsValue, context: JsValue, options: JsValue, sink: EventSink) -> Result<(), JsValue> {
	let parsed = super::model(&model)?;
	let context = super::transcript(context)?;
	let client = Reflect::get(&options, &"client".into())
		.ok()
		.filter(JsValue::is_object)
		.map(|client| Rc::new(JsAnthropicClient { client }) as Rc<dyn AnthropicClientApi>);
	let options = provider_stream_options(options, &model).map(|options| AnthropicOptions {
		client,
		..AnthropicOptions::from_provider_options(options)
	});
	crate::events::send(anthropic_messages::stream(&parsed, &context, options), sink);
	Ok(())
}

/// The module's own implementation as `ProviderStreams`, behind its `streamSimple` export.
#[wasm_bindgen(js_name = aiAnthropicMessagesStreams)]
pub fn streams() -> NativeStreams {
	NativeStreams::new(Rc::new(anthropic_messages::AnthropicMessagesStreams))
}

/// `anthropicMessagesApi()`: the module as lazily loaded `ProviderStreams`.
#[wasm_bindgen(js_name = aiAnthropicMessagesApi)]
pub fn api() -> NativeStreams {
	NativeStreams::new(anthropic_messages_lazy::anthropic_messages_api())
}

/// A JS SDK client passed as `options.client`. The core calls
/// `client.beta.messages.create(params, requestOptions).asResponse()` as the TS does, with `requestOptions` holding
/// the `anthropic-version` header, `signal`, `timeout` and `maxRetries: 0` the core chose. A thrown SDK error keeps
/// its `status`, `type` and `headers`, so the strict-tool fallback sees it as an `APIError`.
struct JsAnthropicClient {
	client: JsValue,
}

impl AnthropicClientApi for JsAnthropicClient {
	fn beta_messages_create<'a>(
		&'a self,
		params: &'a Value,
		options: RequestOptions,
	) -> LocalBoxFuture<'a, PiResult<HttpResponse>> {
		Box::pin(async move {
			let messages = property(&property(&self.client, "beta"), "messages");
			let request_options = Object::new();
			let set = |key: &str, value: &JsValue| {
				let _ = Reflect::set(&request_options, &key.into(), value);
			};
			if let Some(headers) = &options.headers {
				set("headers", &crate::json::to_js(headers)?);
			}
			let signal = options.signal.as_ref().map(crate::abort::to_js);
			if let Some(signal) = &signal {
				set("signal", &signal.js);
			}
			if let Some(timeout) = options.timeout {
				set("timeout", &timeout.into());
			}
			if let Some(max_retries) = options.max_retries {
				set("maxRetries", &max_retries.into());
			}
			let create: Function = property(&messages, "create").unchecked_into();
			let request = create
				.call2(&messages, &crate::json::to_js(params)?, &request_options)
				.map_err(crate::error::from_js)?;
			let as_response: Function = property(&request, "asResponse").unchecked_into();
			let pending = as_response.call0(&request).map_err(crate::error::from_js)?;
			let response = JsFuture::from(Promise::resolve(&pending))
				.await
				.map_err(crate::error::from_js)?;
			Ok(crate::fetch::read_response(&response, signal))
		})
	}
}

/// `object[key]`, `undefined` when it cannot be read.
fn property(object: &JsValue, key: &str) -> JsValue {
	Reflect::get(object, &key.into()).unwrap_or(JsValue::UNDEFINED)
}
