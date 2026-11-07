//! `pi_host::server` over the host's `listen` (`node:http` in `@at-inc/pi-core`): the Rust handler is a JS function
//! that takes `{ method, url }` and resolves with `{ status, headers, body }`, and the server it resolves with is
//! closed through its `close` and `closeAllConnections` methods.

use std::rc::Rc;

use js_sys::{Object, Promise, Reflect};
use pi_host::server::{HttpServer, HttpServerErrorHandler, HttpServerHandler, HttpServerRequest};
use pi_host::{LocalBoxFuture, PiResult};
use wasm_bindgen::JsCast;
use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::JsFuture;

#[wasm_bindgen]
extern "C" {
	type JsServer;
	#[wasm_bindgen(method, getter)]
	fn port(this: &JsServer) -> f64;
	#[wasm_bindgen(method)]
	fn close(this: &JsServer);
	#[wasm_bindgen(method, js_name = closeAllConnections)]
	fn close_all_connections(this: &JsServer);
}

/// The JS `listen` arguments for a Rust handler: the handler and the error listener as JS functions. They are owned
/// by the JS server, which drops them when it is collected.
pub fn callbacks(handler: HttpServerHandler, on_error: HttpServerErrorHandler) -> (JsValue, JsValue) {
	let handler = Closure::<dyn Fn(JsValue) -> Promise>::new(move |request: JsValue| {
		let field = |name: &str| {
			Reflect::get(&request, &name.into())
				.ok()
				.and_then(|value| value.as_string())
				.unwrap_or_default()
		};
		let response = handler(HttpServerRequest {
			method: field("method"),
			target: field("url"),
		});
		wasm_bindgen_futures::future_to_promise(async move {
			let response = response.await;
			let headers = Object::new();
			for (name, value) in &response.headers {
				Reflect::set(&headers, &name.as_str().into(), &value.as_str().into())?;
			}
			let object = Object::new();
			Reflect::set(&object, &"status".into(), &response.status.into())?;
			Reflect::set(&object, &"headers".into(), &headers)?;
			Reflect::set(&object, &"body".into(), &response.body.as_str().into())?;
			Ok(object.into())
		})
	})
	.into_js_value();
	let on_error =
		Closure::<dyn Fn(JsValue)>::new(move |error: JsValue| on_error(crate::error::from_js(error))).into_js_value();
	(handler, on_error)
}

/// The Rust server for the promise the host's `listen` returned.
pub fn listening(listen: Result<Promise, JsValue>) -> LocalBoxFuture<'static, PiResult<Rc<dyn HttpServer>>> {
	Box::pin(async move {
		let promise = listen.map_err(crate::error::from_js)?;
		let server = JsFuture::from(promise).await.map_err(crate::error::from_js)?;
		Ok(Rc::new(server.unchecked_into::<JsServer>()) as Rc<dyn HttpServer>)
	})
}

impl HttpServer for JsServer {
	fn port(&self) -> u16 {
		JsServer::port(self) as u16
	}

	fn close(&self) {
		JsServer::close(self);
	}

	fn close_all_connections(&self) {
		JsServer::close_all_connections(self);
	}
}
