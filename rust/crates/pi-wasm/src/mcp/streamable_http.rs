//! Exports of `transports/streamable-http`: `new StreamableHttpTransport(options)` over the Rust transport, whose
//! requests go through `options.fetch` or the host's `fetch`, and `consumeSseStream` over a JS `ReadableStream`.

use js_sys::{Function, Promise};
use pi_mcp::transports::streamable_http::{
	ConsumeSseOptions, SseEvent, StreamableHttpReconnectOptions, consume_sse_stream,
};
use pi_mcp::{StreamableHttpTransport, StreamableHttpTransportOptions};
use wasm_bindgen::prelude::*;

use super::auth_provider::AuthProviderHandle;
use super::transport::{McpTransportHandle, TransportKind};
use super::{field, function, optional};

/// `new StreamableHttpTransport(options)`. `auth_native` is the Rust provider of an `adaptOAuthProvider` result;
/// `auth_js` an `AuthProvider` written in JS, which the facade wraps (see `auth_provider::from_js`).
#[wasm_bindgen(js_name = mcpCreateStreamableHttpTransport)]
pub fn create_streamable_http_transport(
	options: JsValue,
	auth_native: Option<AuthProviderHandle>,
	auth_js: JsValue,
) -> Result<McpTransportHandle, JsValue> {
	let auth_provider = match auth_native {
		Some(handle) => Some(handle.into_inner()),
		None if auth_js.is_object() => Some(super::auth_provider::from_js(auth_js)),
		None => None,
	};
	let reconnect = field(&options, "reconnect");
	let reconnect = if reconnect.is_object() {
		Some(StreamableHttpReconnectOptions {
			initial_delay_ms: optional(&reconnect, "initialDelayMs")?,
			max_delay_ms: optional(&reconnect, "maxDelayMs")?,
			max_retries: optional(&reconnect, "maxRetries")?,
		})
	} else {
		None
	};
	let transport = StreamableHttpTransport::new(StreamableHttpTransportOptions {
		url: super::url_string(&field(&options, "url")).unwrap_or_default(),
		headers: optional(&options, "headers")?,
		fetch: super::fetch(&options),
		open_get_stream: optional(&options, "openGetStream")?,
		max_message_bytes: optional(&options, "maxMessageBytes")?,
		auth_provider,
		reconnect,
	});
	Ok(McpTransportHandle::new(TransportKind::StreamableHttp(
		crate::options::throw(transport)?,
	)))
}

/// `consumeSseStream(stream, options)`: `onEvent`, `onId` and `onRetry` are called as methods of `options`.
#[wasm_bindgen(js_name = mcpConsumeSseStream)]
pub fn consume_sse(stream: JsValue, options: JsValue) -> Promise {
	super::rejecting(|| {
		let max_event_bytes = optional(&options, "maxEventBytes")?;
		let body = crate::fetch::readable_stream_body(&stream, None);
		let (on_event, on_id, on_retry) = (
			function(&options, "onEvent"),
			function(&options, "onId"),
			function(&options, "onRetry"),
		);
		let call = move |listener: &Option<Function>, value: JsValue| {
			if let Some(listener) = listener {
				let _ = listener.call1(&options, &value);
			}
		};
		Ok(super::unit_promise(async move {
			let options = ConsumeSseOptions {
				max_event_bytes,
				on_event: Some(Box::new(|event: SseEvent| {
					if let Ok(event) = crate::json::to_js(&event) {
						call(&on_event, event);
					}
				})),
				on_id: Some(Box::new(|id: &str| call(&on_id, id.into()))),
				on_retry: Some(Box::new(|delay_ms: f64| call(&on_retry, delay_ms.into()))),
			};
			consume_sse_stream(body, options).await
		}))
	})
}
