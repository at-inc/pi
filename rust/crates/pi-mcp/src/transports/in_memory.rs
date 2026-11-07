//! Port of `packages/mcp/src/transports/in-memory.ts`.
//! PORT-STATUS: reviewed
//!
//! `InMemoryTransport` is a cheap handle (`Clone` shares the transport). Each side holds its peer strongly, as
//! in TS; closing a side drops its reference to the peer, so a closed pair does not keep itself alive.
//! `queueMicrotask` becomes `pi_host::spawn_local`, and `structuredClone` a `Clone` of the message.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use futures::channel::oneshot;
use pi_host::{LocalBoxFuture, PiError, PiResult};

use super::transport::{
	McpTransport, McpTransportCloseListener, McpTransportErrorListener, McpTransportMessageListener,
	McpTransportSendOptions, TransportEvents, Unsubscribe,
};
use crate::protocol::jsonrpc::{JsonRpcMessage, McpConnectionClosedError};

struct Inner {
	events: TransportEvents,
	peer: RefCell<Option<InMemoryTransport>>,
	started: Cell<bool>,
	closed: Cell<bool>,
}

#[derive(Clone)]
pub struct InMemoryTransport {
	inner: Rc<Inner>,
}

impl Default for InMemoryTransport {
	fn default() -> Self {
		Self::new()
	}
}

impl InMemoryTransport {
	/// `new InMemoryTransport()`.
	pub fn new() -> Self {
		Self {
			inner: Rc::new(Inner {
				events: TransportEvents::new(),
				peer: RefCell::new(None),
				started: Cell::new(false),
				closed: Cell::new(false),
			}),
		}
	}

	pub fn connect_peer(&self, peer: &InMemoryTransport) -> PiResult<()> {
		if self.inner.peer.borrow().is_some() {
			return Err(PiError::new("In-memory MCP transport already has a peer"));
		}
		*self.inner.peer.borrow_mut() = Some(peer.clone());
		Ok(())
	}

	/// Exposed so tests can simulate transport-level failures.
	pub fn emit_error(&self, error: &PiError) {
		self.inner.events.emit_error(error);
	}

	fn deliver(&self, message: &JsonRpcMessage) {
		if self.inner.closed.get() {
			return;
		}
		self.inner.events.emit_message(message);
	}
}

impl McpTransport for InMemoryTransport {
	fn start(&self) -> LocalBoxFuture<'static, PiResult<()>> {
		let result = if self.inner.closed.get() {
			Err(McpConnectionClosedError::new())
		} else {
			self.inner.started.set(true);
			Ok(())
		};
		Box::pin(async move { result })
	}

	fn send(
		&self,
		message: JsonRpcMessage,
		_options: McpTransportSendOptions,
	) -> LocalBoxFuture<'static, PiResult<()>> {
		let result = (|| {
			if !self.inner.started.get() || self.inner.closed.get() {
				return Err(McpConnectionClosedError::new());
			}
			let peer = self.inner.peer.borrow().clone();
			let Some(peer) = peer.filter(|peer| peer.inner.started.get() && !peer.inner.closed.get()) else {
				return Err(McpConnectionClosedError::with_message(
					"In-memory MCP peer is not connected",
				));
			};
			let (delivered, on_delivered) = oneshot::channel::<()>();
			pi_host::spawn_local(async move {
				peer.deliver(&message);
				let _ = delivered.send(());
			});
			Ok(on_delivered)
		})();
		Box::pin(async move {
			// The awaiting sender resumes after the delivery, as a JS continuation runs after the earlier microtask.
			let _ = result?.await;
			Ok(())
		})
	}

	fn close(&self) -> LocalBoxFuture<'static, PiResult<()>> {
		if self.inner.closed.get() {
			return Box::pin(async { Ok(()) });
		}
		self.inner.closed.set(true);
		self.inner.events.emit_close();
		let peer = self.inner.peer.borrow_mut().take();
		match peer {
			Some(peer) => peer.close(),
			None => Box::pin(async { Ok(()) }),
		}
	}

	fn on_message(&self, listener: McpTransportMessageListener) -> Unsubscribe {
		self.inner.events.on_message(listener)
	}

	fn on_error(&self, listener: McpTransportErrorListener) -> Unsubscribe {
		self.inner.events.on_error(listener)
	}

	fn on_close(&self, listener: McpTransportCloseListener) -> Unsubscribe {
		self.inner.events.on_close(listener)
	}
}

/// The two connected ends of an in-memory transport.
pub struct InMemoryTransportPair {
	pub client: InMemoryTransport,
	pub server: InMemoryTransport,
}

pub fn create_in_memory_transport_pair() -> InMemoryTransportPair {
	let client = InMemoryTransport::new();
	let server = InMemoryTransport::new();
	client.connect_peer(&server).expect("a new transport has no peer");
	server.connect_peer(&client).expect("a new transport has no peer");
	InMemoryTransportPair { client, server }
}
