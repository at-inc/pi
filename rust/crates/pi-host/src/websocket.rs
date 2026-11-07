//! WebSocket client capability, the counterpart of the WHATWG `WebSocket` global that ported code constructs with
//! `new WebSocket(url, { headers })` (Node's undici and Bun accept the `headers` option).
//!
//! A socket is an event target: listeners added with `add_event_listener` receive every `open`, `message`, `error` and
//! `close` event dispatched while they are registered, and events dispatched while no listener is registered are lost,
//! as with `addEventListener`. The host connects in the background after `connect` returns, like the constructor.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use indexmap::IndexMap;

use crate::error::PiResult;

/// `WebSocket.CONNECTING`.
pub const CONNECTING: u16 = 0;
/// `WebSocket.OPEN`.
pub const OPEN: u16 = 1;
/// `WebSocket.CLOSING`.
pub const CLOSING: u16 = 2;
/// `WebSocket.CLOSED`.
pub const CLOSED: u16 = 3;

/// `MessageEvent.data`: a text frame is a string, a binary frame its bytes.
#[derive(Clone, Debug, PartialEq)]
pub enum WebSocketData {
	Text(String),
	Binary(Vec<u8>),
}

/// An event a socket dispatches.
#[derive(Clone, Debug, PartialEq)]
pub enum WebSocketEvent {
	Open,
	Message {
		data: WebSocketData,
	},
	/// `error`: `message` is the runtime's `ErrorEvent.message` when it provides one.
	Error {
		message: Option<String>,
	},
	Close {
		code: u16,
		reason: String,
		was_clean: bool,
	},
}

/// An event listener.
pub type WebSocketListener = Rc<dyn Fn(&WebSocketEvent)>;

/// Identifies a registered listener for `remove_event_listener`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct WebSocketListenerId(u64);

/// A client socket.
pub trait WebSocket {
	/// `socket.readyState`; `None` for a socket that does not expose one.
	fn ready_state(&self) -> Option<u16>;
	/// `socket.send(text)`. Throws (`Err`) while the socket is still connecting.
	fn send(&self, data: &str) -> PiResult<()>;
	/// `socket.close(code, reason)`.
	fn close(&self, code: u16, reason: &str) -> PiResult<()>;
	/// `socket.addEventListener(type, listener)` for every event type at once.
	fn add_event_listener(&self, listener: WebSocketListener) -> WebSocketListenerId;
	/// `socket.removeEventListener(type, listener)`.
	fn remove_event_listener(&self, id: WebSocketListenerId);
}

/// `new WebSocket(url, { headers })` on the current host. `None` when the runtime has no WebSocket
/// (`typeof globalThis.WebSocket !== "function"`); `Err` when the constructor throws.
pub fn connect(url: &str, headers: &IndexMap<String, String>) -> Option<PiResult<Rc<dyn WebSocket>>> {
	crate::host::current().websocket(url, headers)
}

/// The listener list of a socket, shared by host implementations.
#[derive(Default)]
pub struct WebSocketListeners {
	next_id: Cell<u64>,
	listeners: RefCell<Vec<(WebSocketListenerId, WebSocketListener)>>,
}

impl WebSocketListeners {
	/// Registers a listener.
	pub fn add(&self, listener: WebSocketListener) -> WebSocketListenerId {
		let id = WebSocketListenerId(self.next_id.get());
		self.next_id.set(id.0 + 1);
		self.listeners.borrow_mut().push((id, listener));
		id
	}

	/// Unregisters a listener; unknown ids are ignored.
	pub fn remove(&self, id: WebSocketListenerId) {
		self.listeners.borrow_mut().retain(|(existing, _)| *existing != id);
	}

	/// Calls every listener registered when the dispatch starts, in registration order. A listener removed by an
	/// earlier listener of the same dispatch is skipped, like `EventTarget`.
	pub fn dispatch(&self, event: &WebSocketEvent) {
		let snapshot: Vec<(WebSocketListenerId, WebSocketListener)> = self.listeners.borrow().clone();
		for (id, listener) in snapshot {
			let registered = self.listeners.borrow().iter().any(|(existing, _)| *existing == id);
			if registered {
				listener(event);
			}
		}
	}
}
