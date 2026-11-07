//! `pi_host::websocket::WebSocket` over a JS `WebSocket` from the host: the four socket events are forwarded to the
//! Rust listeners, and `send`/`close` call the JS socket.

use std::rc::Rc;

use js_sys::{ArrayBuffer, Reflect, Uint8Array};
use pi_host::PiResult;
use pi_host::websocket::{
	WebSocket, WebSocketData, WebSocketEvent, WebSocketListener, WebSocketListenerId, WebSocketListeners,
};
use wasm_bindgen::JsCast;
use wasm_bindgen::prelude::*;

#[wasm_bindgen]
extern "C" {
	type JsSocket;
	#[wasm_bindgen(method, getter, js_name = readyState)]
	fn ready_state(this: &JsSocket) -> Option<f64>;
	#[wasm_bindgen(method, setter, js_name = binaryType)]
	fn set_binary_type(this: &JsSocket, value: &str);
	#[wasm_bindgen(method, catch)]
	fn send(this: &JsSocket, data: &str) -> Result<(), JsValue>;
	#[wasm_bindgen(method, catch)]
	fn close(this: &JsSocket, code: u16, reason: &str) -> Result<(), JsValue>;
	#[wasm_bindgen(method, js_name = addEventListener)]
	fn add_event_listener(this: &JsSocket, kind: &str, listener: &JsValue);
	#[wasm_bindgen(method, js_name = removeEventListener)]
	fn remove_event_listener(this: &JsSocket, kind: &str, listener: &JsValue);
}

const EVENTS: [&str; 4] = ["open", "message", "error", "close"];

/// The JS listener that forwards one socket event kind.
type Forwarder = Closure<dyn FnMut(JsValue)>;

/// A JS socket and the JS listeners that forward its events.
pub struct JsWebSocket {
	socket: JsSocket,
	listeners: Rc<WebSocketListeners>,
	forwarders: Vec<(&'static str, Forwarder)>,
}

impl JsWebSocket {
	/// Wraps a socket the host just constructed and starts forwarding its events.
	pub fn open(socket: JsValue) -> Rc<dyn WebSocket> {
		let socket: JsSocket = socket.unchecked_into();
		socket.set_binary_type("arraybuffer");
		let listeners = Rc::new(WebSocketListeners::default());
		let forwarders = EVENTS
			.iter()
			.map(|&kind| {
				let listeners = listeners.clone();
				let forwarder = Forwarder::new(move |event: JsValue| {
					listeners.dispatch(&socket_event(kind, &event));
				});
				socket.add_event_listener(kind, forwarder.as_ref());
				(kind, forwarder)
			})
			.collect();
		Rc::new(JsWebSocket {
			socket,
			listeners,
			forwarders,
		})
	}
}

impl Drop for JsWebSocket {
	fn drop(&mut self) {
		for (kind, forwarder) in &self.forwarders {
			self.socket.remove_event_listener(kind, forwarder.as_ref());
		}
	}
}

fn socket_event(kind: &str, event: &JsValue) -> WebSocketEvent {
	let field = |name: &str| Reflect::get(event, &name.into()).unwrap_or(JsValue::UNDEFINED);
	match kind {
		"open" => WebSocketEvent::Open,
		"message" => {
			let data = field("data");
			let data = match data.as_string() {
				Some(text) => WebSocketData::Text(text),
				None if data.is_instance_of::<ArrayBuffer>() => WebSocketData::Binary(Uint8Array::new(&data).to_vec()),
				None => WebSocketData::Binary(data.unchecked_into::<Uint8Array>().to_vec()),
			};
			WebSocketEvent::Message { data }
		}
		"error" => WebSocketEvent::Error {
			message: field("message").as_string(),
		},
		_ => WebSocketEvent::Close {
			code: field("code").as_f64().unwrap_or(1005.0) as u16,
			reason: field("reason").as_string().unwrap_or_default(),
			was_clean: field("wasClean").as_bool().unwrap_or(false),
		},
	}
}

impl WebSocket for JsWebSocket {
	fn ready_state(&self) -> Option<u16> {
		self.socket.ready_state().map(|state| state as u16)
	}

	fn send(&self, data: &str) -> PiResult<()> {
		self.socket.send(data).map_err(crate::error::from_js)
	}

	fn close(&self, code: u16, reason: &str) -> PiResult<()> {
		self.socket.close(code, reason).map_err(crate::error::from_js)
	}

	fn add_event_listener(&self, listener: WebSocketListener) -> WebSocketListenerId {
		self.listeners.add(listener)
	}

	fn remove_event_listener(&self, id: WebSocketListenerId) {
		self.listeners.remove(id);
	}
}
