//! `pi_host::http_server::HttpServer` over the host's `listenHttp` (`PiHttpServer`): the Rust handler is a JS
//! function from a `PiServerRequest` to a Promise of a `PiServerResponse`.

use std::rc::Rc;

use indexmap::IndexMap;
use js_sys::{Object, Promise, Reflect};
use pi_host::http_server::{HttpServer, ServerHandler, ServerRequest};
use pi_host::{LocalBoxFuture, PiResult};
use wasm_bindgen::JsCast;
use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::JsFuture;

#[wasm_bindgen]
extern "C" {
	type JsServer;
	#[wasm_bindgen(method, getter)]
	fn port(this: &JsServer) -> f64;
	#[wasm_bindgen(method, catch)]
	fn close(this: &JsServer) -> Result<Promise, JsValue>;
}

/// The JS request handler over `handler`.
pub fn handler_to_js(handler: ServerHandler) -> JsValue {
	Closure::<dyn Fn(JsValue) -> Promise>::new(move |request: JsValue| {
		let field = |name: &str| Reflect::get(&request, &name.into()).unwrap_or(JsValue::UNDEFINED);
		let headers: IndexMap<String, String> = crate::json::from_js(field("headers")).unwrap_or_default();
		let pending = handler(ServerRequest {
			method: field("method").as_string().unwrap_or_default(),
			url: field("url").as_string().unwrap_or_default(),
			headers,
		});
		wasm_bindgen_futures::future_to_promise(async move {
			let response = pending.await;
			let object = Object::new();
			let _ = Reflect::set(&object, &"status".into(), &JsValue::from(response.status));
			let headers = crate::json::to_js(&response.headers).map_err(|error| crate::error::to_js(&error))?;
			let _ = Reflect::set(&object, &"headers".into(), &headers);
			let _ = Reflect::set(&object, &"body".into(), &response.body.as_str().into());
			Ok(object.into())
		})
	})
	.into_js_value()
}

/// The server of a `listenHttp` call: `undefined` is a host without a listener.
pub fn listening(listening: Result<JsValue, JsValue>) -> LocalBoxFuture<'static, PiResult<Rc<dyn HttpServer>>> {
	Box::pin(async move {
		let pending = listening.map_err(crate::error::from_js)?;
		if pending.is_undefined() || pending.is_null() {
			return Err(pi_host::http_server::listen_unsupported());
		}
		let server = JsFuture::from(Promise::resolve(&pending))
			.await
			.map_err(crate::error::from_js)?;
		Ok(Rc::new(JsHttpServer {
			server: server.unchecked_into(),
		}) as Rc<dyn HttpServer>)
	})
}

struct JsHttpServer {
	server: JsServer,
}

impl HttpServer for JsHttpServer {
	fn port(&self) -> u16 {
		self.server.port() as u16
	}

	fn close(&self) -> LocalBoxFuture<'static, PiResult<()>> {
		let closed = self.server.close();
		Box::pin(async move {
			let promise = closed.map_err(crate::error::from_js)?;
			JsFuture::from(promise).await.map_err(crate::error::from_js)?;
			Ok(())
		})
	}
}
