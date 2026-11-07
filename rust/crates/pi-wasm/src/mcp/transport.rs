//! Transports across the boundary: `McpTransportHandle` is a Rust transport that the facade's transport classes own
//! and JS code calls, and `JsTransport` is a transport written in JS (any object with the `McpTransport` methods)
//! that the Rust client calls.

use std::rc::Rc;

use js_sys::{Function, Object, Promise, Reflect};
use pi_host::{LocalBoxFuture, PiError, PiResult};
use pi_mcp::testing::InMemoryTransport;
use pi_mcp::transports::transport::{
	McpTransportCloseListener, McpTransportErrorListener, McpTransportMessageListener, Unsubscribe,
};
use pi_mcp::{
	JsonRpcMessage, McpTransport, McpTransportSendOptions, StdioTransport, StreamableHttpTransport,
	parse_json_rpc_message,
};
use serde_json::Value;
use wasm_bindgen::JsCast;
use wasm_bindgen::prelude::*;

/// The Rust transport behind a handle, for the methods only one kind has.
#[derive(Clone)]
pub enum TransportKind {
	Stdio(StdioTransport),
	StreamableHttp(StreamableHttpTransport),
	InMemory(InMemoryTransport),
}

/// A Rust transport, owned by the facade's `StdioTransport`, `StreamableHttpTransport` or `InMemoryTransport`.
#[wasm_bindgen]
pub struct McpTransportHandle {
	inner: Rc<dyn McpTransport>,
	kind: TransportKind,
}

impl McpTransportHandle {
	pub fn new(kind: TransportKind) -> Self {
		let inner: Rc<dyn McpTransport> = match kind.clone() {
			TransportKind::Stdio(transport) => Rc::new(transport),
			TransportKind::StreamableHttp(transport) => Rc::new(transport),
			TransportKind::InMemory(transport) => Rc::new(transport),
		};
		Self { inner, kind }
	}
}

#[wasm_bindgen]
impl McpTransportHandle {
	/// Another handle to the same transport, for passing by value.
	pub fn share(&self) -> McpTransportHandle {
		McpTransportHandle {
			inner: self.inner.clone(),
			kind: self.kind.clone(),
		}
	}

	/// `transport.start()`.
	pub fn start(&self) -> Promise {
		super::unit_promise(self.inner.start())
	}

	/// `transport.send(message, { signal })`.
	pub fn send(&self, message: JsValue, options: JsValue) -> Promise {
		super::rejecting(|| {
			let message = crate::options::throw(message_from_js(message))?;
			let send = self.inner.send(
				message,
				McpTransportSendOptions {
					signal: super::signal(&options),
				},
			);
			Ok(super::unit_promise(send))
		})
	}

	/// `transport.close()`.
	pub fn close(&self) -> Promise {
		super::unit_promise(self.inner.close())
	}

	/// `transport.onMessage(listener)`.
	#[wasm_bindgen(js_name = onMessage)]
	pub fn on_message(&self, listener: Function) -> JsValue {
		super::unsubscribe_to_js(self.inner.on_message(Rc::new(move |message: &JsonRpcMessage| {
			if let Ok(message) = crate::json::to_js(&message.to_value()) {
				let _ = listener.call1(&JsValue::UNDEFINED, &message);
			}
		})))
	}

	/// `transport.onError(listener)`.
	#[wasm_bindgen(js_name = onError)]
	pub fn on_error(&self, listener: Function) -> JsValue {
		super::unsubscribe_to_js(self.inner.on_error(Rc::new(move |error: &PiError| {
			let _ = listener.call1(&JsValue::UNDEFINED, &crate::error::to_js(error));
		})))
	}

	/// `transport.onClose(listener)`.
	#[wasm_bindgen(js_name = onClose)]
	pub fn on_close(&self, listener: Function) -> JsValue {
		super::unsubscribe_to_js(self.inner.on_close(Rc::new(move || {
			let _ = listener.call0(&JsValue::UNDEFINED);
		})))
	}

	/// `transport.setProtocolVersion(version)`.
	#[wasm_bindgen(js_name = setProtocolVersion)]
	pub fn set_protocol_version(&self, version: &str) {
		self.inner.set_protocol_version(version);
	}

	/// `StdioTransport.pid`.
	pub fn pid(&self) -> Option<f64> {
		match &self.kind {
			TransportKind::Stdio(transport) => transport.pid().map(f64::from),
			_ => None,
		}
	}

	/// `StdioTransport.stderr`.
	pub fn stderr(&self) -> String {
		match &self.kind {
			TransportKind::Stdio(transport) => transport.stderr(),
			_ => String::new(),
		}
	}

	/// `StreamableHttpTransport.url`, as a string.
	pub fn url(&self) -> Option<String> {
		match &self.kind {
			TransportKind::StreamableHttp(transport) => Some(transport.url().to_string()),
			_ => None,
		}
	}

	/// `StreamableHttpTransport.sessionId`.
	#[wasm_bindgen(js_name = sessionId)]
	pub fn session_id(&self) -> Option<String> {
		match &self.kind {
			TransportKind::StreamableHttp(transport) => transport.session_id(),
			_ => None,
		}
	}

	/// `InMemoryTransport.connectPeer(peer)`.
	#[wasm_bindgen(js_name = connectPeer)]
	pub fn connect_peer(&self, peer: &McpTransportHandle) -> Result<(), JsValue> {
		let (TransportKind::InMemory(transport), TransportKind::InMemory(peer)) = (&self.kind, &peer.kind) else {
			return Err(crate::error::to_js(&PiError::type_error(
				"connectPeer needs two in-memory MCP transports",
			)));
		};
		crate::options::throw(transport.connect_peer(peer))
	}

	/// `InMemoryTransport.emitError(error)`.
	#[wasm_bindgen(js_name = emitError)]
	pub fn emit_error(&self, error: JsValue) {
		if let TransportKind::InMemory(transport) = &self.kind {
			transport.emit_error(&crate::error::from_js(error));
		}
	}
}

/// The transport a JS caller passed: the Rust transport of a facade transport, or the JS object itself.
pub fn from_js(native: Option<McpTransportHandle>, js: JsValue) -> Rc<dyn McpTransport> {
	match native {
		Some(handle) => handle.inner,
		None => Rc::new(JsTransport { object: js }),
	}
}

/// A JSON-RPC message from JS data, checked like `parseJsonRpcMessage`.
fn message_from_js(message: JsValue) -> PiResult<JsonRpcMessage> {
	parse_json_rpc_message(crate::json::from_js::<Value>(message)?)
}

/// A transport written in JS. Its methods are looked up at call time and called with the object as `this`.
struct JsTransport {
	object: JsValue,
}

impl JsTransport {
	fn call(&self, method: &'static str, args: Vec<JsValue>) -> LocalBoxFuture<'static, PiResult<()>> {
		let function = super::function(&self.object, method);
		let object = self.object.clone();
		Box::pin(async move {
			let Some(function) = function else {
				return Err(PiError::type_error(format!("transport.{method} is not a function")));
			};
			crate::callback::call(&function, &object, &args).await?;
			Ok(())
		})
	}

	/// `object[method](listener)`, returning the unsubscribe function it gives back.
	fn subscribe(&self, method: &str, listener: JsValue) -> Unsubscribe {
		let unsubscribe = super::function(&self.object, method)
			.and_then(|subscribe| subscribe.call1(&self.object, &listener).ok())
			.and_then(|unsubscribe| unsubscribe.dyn_into::<Function>().ok());
		Box::new(move || {
			if let Some(unsubscribe) = &unsubscribe {
				let _ = unsubscribe.call0(&JsValue::UNDEFINED);
			}
		})
	}
}

impl McpTransport for JsTransport {
	fn start(&self) -> LocalBoxFuture<'static, PiResult<()>> {
		self.call("start", Vec::new())
	}

	fn send(&self, message: JsonRpcMessage, options: McpTransportSendOptions) -> LocalBoxFuture<'static, PiResult<()>> {
		let message = crate::json::to_js(&message.to_value());
		let signal = options.signal.as_ref().map(crate::abort::to_js);
		let send_options = Object::new();
		if let Some(signal) = &signal {
			let _ = Reflect::set(&send_options, &"signal".into(), &signal.js);
		}
		let send = message.map(|message| self.call("send", vec![message, send_options.into()]));
		Box::pin(async move {
			// The JS signal follows the Rust one until the send settles.
			let _signal = signal;
			send?.await
		})
	}

	fn close(&self) -> LocalBoxFuture<'static, PiResult<()>> {
		self.call("close", Vec::new())
	}

	fn on_message(&self, listener: McpTransportMessageListener) -> Unsubscribe {
		let forward = Closure::<dyn Fn(JsValue)>::new(move |message: JsValue| {
			if let Ok(message) = message_from_js(message) {
				listener(&message);
			}
		});
		self.subscribe("onMessage", forward.into_js_value())
	}

	fn on_error(&self, listener: McpTransportErrorListener) -> Unsubscribe {
		let forward = Closure::<dyn Fn(JsValue)>::new(move |error: JsValue| listener(&crate::error::from_js(error)));
		self.subscribe("onError", forward.into_js_value())
	}

	fn on_close(&self, listener: McpTransportCloseListener) -> Unsubscribe {
		let forward = Closure::<dyn Fn()>::new(move || listener());
		self.subscribe("onClose", forward.into_js_value())
	}

	fn set_protocol_version(&self, version: &str) {
		if let Some(set) = super::function(&self.object, "setProtocolVersion") {
			let _ = set.call1(&self.object, &version.into());
		}
	}
}
