//! `AuthProvider` across the boundary: `AuthProviderHandle` is a Rust provider (an `adaptOAuthProvider` result)
//! that JS code and the facade transports use, and `from_js` makes a Rust provider of one written in JS. The
//! `UnauthorizedContext` crosses with its response as a JS `Response` whose body streams the Rust body.

use std::cell::RefCell;
use std::rc::Rc;

use futures::StreamExt;
use js_sys::{Object, Promise, Reflect, Uint8Array};
use pi_host::{Body, HttpResponse, PiError, PiResult};
use pi_mcp::auth_provider::OnUnauthorizedFunction;
use pi_mcp::{AuthProvider, UnauthorizedContext};
use wasm_bindgen::JsCast;
use wasm_bindgen::prelude::*;

use super::{field, function};

#[wasm_bindgen]
extern "C" {
	#[wasm_bindgen(js_name = Response)]
	type JsResponse;
	#[wasm_bindgen(constructor, catch, js_class = "Response")]
	fn new(body: &JsValue, init: &JsValue) -> Result<JsResponse, JsValue>;

	#[wasm_bindgen(js_name = ReadableStream)]
	type JsReadableStream;
	#[wasm_bindgen(constructor, catch, js_class = "ReadableStream")]
	fn new(source: &JsValue) -> Result<JsReadableStream, JsValue>;

	type StreamController;
	#[wasm_bindgen(method)]
	fn enqueue(this: &StreamController, chunk: &Uint8Array);
	#[wasm_bindgen(method)]
	fn close(this: &StreamController);
}

/// A Rust `AuthProvider`, owned by the facade's `adaptOAuthProvider` result.
#[wasm_bindgen]
pub struct AuthProviderHandle {
	inner: AuthProvider,
}

impl AuthProviderHandle {
	pub fn new(inner: AuthProvider) -> Self {
		Self { inner }
	}

	pub fn into_inner(self) -> AuthProvider {
		self.inner
	}
}

#[wasm_bindgen]
impl AuthProviderHandle {
	/// Another handle to the same provider, for passing by value.
	pub fn share(&self) -> AuthProviderHandle {
		AuthProviderHandle {
			inner: self.inner.clone(),
		}
	}

	/// `provider.token()`.
	pub fn token(&self) -> Promise {
		super::optional_promise((self.inner.token)())
	}

	/// Whether the provider has `onUnauthorized`.
	#[wasm_bindgen(js_name = hasOnUnauthorized)]
	pub fn has_on_unauthorized(&self) -> bool {
		self.inner.on_unauthorized.is_some()
	}

	/// `provider.onUnauthorized({ response, serverUrl, fetch, token })` with a JS `Response`.
	#[wasm_bindgen(js_name = onUnauthorized)]
	pub fn on_unauthorized(&self, context: JsValue) -> Promise {
		let on_unauthorized = self.inner.on_unauthorized.clone();
		super::rejecting(|| {
			let Some(on_unauthorized) = on_unauthorized else {
				return Ok(Promise::resolve(&JsValue::UNDEFINED));
			};
			let server_url = super::url_field(&context, "serverUrl")?
				.ok_or_else(|| crate::error::to_js(&PiError::type_error("Invalid URL: undefined")))?;
			let context = UnauthorizedContext {
				response: crate::fetch::read_response(&field(&context, "response"), None),
				server_url,
				fetch: super::fetch(&context).unwrap_or_else(pi_host::http::global_fetch),
				token: field(&context, "token").as_string(),
			};
			Ok(super::unit_promise(on_unauthorized(context)))
		})
	}
}

/// The Rust provider of an `AuthProvider` written in JS. `onUnauthorized` receives `{ response, serverUrl, token }`
/// with `serverUrl` as a string; the facade's wrapper adds `fetch` and makes `serverUrl` a `URL`.
pub fn from_js(object: JsValue) -> AuthProvider {
	let token = function(&object, "token");
	let this = object.clone();
	let on_unauthorized = function(&object, "onUnauthorized").map(|on_unauthorized| {
		let this = object.clone();
		Rc::new(move |context: UnauthorizedContext| {
			let (on_unauthorized, this) = (on_unauthorized.clone(), this.clone());
			Box::pin(async move {
				let context = context_to_js(context)?;
				crate::callback::call(&on_unauthorized, &this, &[context]).await?;
				Ok(())
			}) as pi_host::LocalBoxFuture<'static, PiResult<()>>
		}) as OnUnauthorizedFunction
	});
	AuthProvider {
		token: Rc::new(move || {
			let (token, this) = (token.clone(), this.clone());
			Box::pin(async move {
				let Some(token) = token else {
					return Err(PiError::type_error("authProvider.token is not a function"));
				};
				Ok(crate::callback::call(&token, &this, &[]).await?.as_string())
			})
		}),
		on_unauthorized,
	}
}

fn context_to_js(context: UnauthorizedContext) -> PiResult<JsValue> {
	let object = Object::new();
	let set = |key: &str, value: &JsValue| {
		let _ = Reflect::set(&object, &key.into(), value);
	};
	set("response", &response_to_js(context.response)?);
	set("serverUrl", &context.server_url.as_str().into());
	if let Some(token) = &context.token {
		set("token", &token.as_str().into());
	}
	Ok(object.into())
}

/// `new Response(body, { status, statusText, headers })` for a Rust response. Statuses that cannot carry a body
/// get none.
pub fn response_to_js(response: HttpResponse) -> PiResult<JsValue> {
	let init = Object::new();
	let _ = Reflect::set(&init, &"status".into(), &response.status.into());
	let _ = Reflect::set(&init, &"statusText".into(), &response.status_text.as_str().into());
	let _ = Reflect::set(&init, &"headers".into(), &crate::json::to_js(&response.headers)?);
	let body = if matches!(response.status, 101 | 103 | 204 | 205 | 304) {
		JsValue::NULL
	} else {
		readable_stream(response.body)?
	};
	JsResponse::new(&body, &init)
		.map(Into::into)
		.map_err(crate::error::from_js)
}

/// A JS `ReadableStream` that pulls the Rust body chunk by chunk; cancelling it drops the body.
fn readable_stream(body: Body) -> PiResult<JsValue> {
	let body = Rc::new(RefCell::new(Some(body)));
	let source = Object::new();
	let pulled = body.clone();
	let pull = Closure::<dyn Fn(JsValue) -> Promise>::new(move |controller: JsValue| {
		let body = pulled.clone();
		wasm_bindgen_futures::future_to_promise(async move {
			let controller: StreamController = controller.unchecked_into();
			// The stream calls `pull` again only after this one settled, so the body is never taken twice.
			let Some(mut stream) = body.borrow_mut().take() else {
				controller.close();
				return Ok(JsValue::UNDEFINED);
			};
			match stream.next().await {
				Some(Ok(chunk)) => {
					controller.enqueue(&Uint8Array::from(&chunk[..]));
					*body.borrow_mut() = Some(stream);
				}
				Some(Err(error)) => return Err(crate::error::to_js(&error)),
				None => controller.close(),
			}
			Ok(JsValue::UNDEFINED)
		})
	});
	let cancel = Closure::<dyn Fn()>::new(move || drop(body.borrow_mut().take()));
	let _ = Reflect::set(&source, &"pull".into(), &pull.into_js_value());
	let _ = Reflect::set(&source, &"cancel".into(), &cancel.into_js_value());
	JsReadableStream::new(&source)
		.map(Into::into)
		.map_err(crate::error::from_js)
}
