//! Port of `packages/mcp/src/transports/transport.ts`.
//! PORT-STATUS: ported
//!
//! Transport methods return `'static` futures, so implementations keep their state behind an `Rc` and clone
//! it into the future. Listener removal returns an `Unsubscribe` closure, the counterpart of the TS
//! `() => void`. Listeners are stored in insertion order like a `Set`; emitting iterates a snapshot and skips
//! listeners removed meanwhile, so a listener added while an event is being emitted first sees the next one
//! (a JS `Set` iteration would also visit it).

use std::cell::{Cell, RefCell};
use std::rc::{Rc, Weak};

use pi_host::{AbortSignal, LocalBoxFuture, PiError, PiResult};

use crate::protocol::jsonrpc::JsonRpcMessage;

pub const DEFAULT_MAX_MESSAGE_BYTES: usize = 16 * 1024 * 1024;

pub type McpTransportMessageListener = Rc<dyn Fn(&JsonRpcMessage)>;
pub type McpTransportErrorListener = Rc<dyn Fn(&PiError)>;
pub type McpTransportCloseListener = Rc<dyn Fn()>;
/// The `() => void` that removes a listener.
pub type Unsubscribe = Box<dyn Fn()>;

#[derive(Clone, Debug, Default)]
pub struct McpTransportSendOptions {
	/// Aborted once the sender no longer waits for a reply (the request was cancelled, timed out, answered,
	/// or the client closed). A transport must not dispatch the message after that, for example when
	/// retrying it after an authorization refresh. It does not undo a message already dispatched.
	pub signal: Option<AbortSignal>,
}

pub trait McpTransport {
	fn start(&self) -> LocalBoxFuture<'static, PiResult<()>>;
	fn send(&self, message: JsonRpcMessage, options: McpTransportSendOptions) -> LocalBoxFuture<'static, PiResult<()>>;
	fn close(&self) -> LocalBoxFuture<'static, PiResult<()>>;
	fn on_message(&self, listener: McpTransportMessageListener) -> Unsubscribe;
	fn on_error(&self, listener: McpTransportErrorListener) -> Unsubscribe;
	fn on_close(&self, listener: McpTransportCloseListener) -> Unsubscribe;
	/// `setProtocolVersion?(version)`; transports without it ignore the call.
	fn set_protocol_version(&self, _version: &str) {}
}

type ListenerList<T> = Rc<RefCell<Vec<(u64, T)>>>;

/// Listener bookkeeping shared by transports. `emitClose` fires at most once per transport.
#[derive(Default)]
pub struct TransportEvents {
	message_listeners: ListenerList<McpTransportMessageListener>,
	error_listeners: ListenerList<McpTransportErrorListener>,
	close_listeners: ListenerList<McpTransportCloseListener>,
	next_listener_id: Rc<Cell<u64>>,
	close_emitted: Cell<bool>,
}

fn add_listener<T: 'static>(list: &ListenerList<T>, next_id: &Cell<u64>, listener: T) -> Unsubscribe {
	let id = next_id.get();
	next_id.set(id + 1);
	list.borrow_mut().push((id, listener));
	let list: Weak<RefCell<Vec<(u64, T)>>> = Rc::downgrade(list);
	Box::new(move || {
		if let Some(list) = list.upgrade() {
			list.borrow_mut().retain(|(listener_id, _)| *listener_id != id);
		}
	})
}

/// Calls `call` for every listener of `list`, skipping ones removed while earlier listeners ran.
pub(crate) fn emit_each<T: Clone>(list: &ListenerList<T>, call: impl Fn(&T)) {
	let snapshot: Vec<(u64, T)> = list.borrow().clone();
	for (id, listener) in snapshot {
		let registered = list.borrow().iter().any(|(listener_id, _)| *listener_id == id);
		if registered {
			call(&listener);
		}
	}
}

impl TransportEvents {
	/// Empty listener sets.
	pub fn new() -> Self {
		Self::default()
	}

	pub fn on_message(&self, listener: McpTransportMessageListener) -> Unsubscribe {
		add_listener(&self.message_listeners, &self.next_listener_id, listener)
	}

	pub fn on_error(&self, listener: McpTransportErrorListener) -> Unsubscribe {
		add_listener(&self.error_listeners, &self.next_listener_id, listener)
	}

	pub fn on_close(&self, listener: McpTransportCloseListener) -> Unsubscribe {
		add_listener(&self.close_listeners, &self.next_listener_id, listener)
	}

	pub fn emit_message(&self, message: &JsonRpcMessage) {
		emit_each(&self.message_listeners, |listener| listener(message));
	}

	pub fn emit_error(&self, error: &PiError) {
		emit_each(&self.error_listeners, |listener| listener(error));
	}

	pub fn emit_close(&self) {
		if self.close_emitted.get() {
			return;
		}
		self.close_emitted.set(true);
		emit_each(&self.close_listeners, |listener| listener());
	}
}
