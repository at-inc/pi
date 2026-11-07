//! Port of `packages/mcp/src/transports/stdio.ts`.
//! PORT-STATUS: reviewed
//!
//! The server runs through the host's `spawn_process` (`pi_host::process`), the counterpart of `cross-spawn`
//! with piped stdio: the native host uses tokio processes in their own process group, the JS host
//! `node:child_process`, and hosts without processes (Workers) fail `start`. Killing the process tree and the
//! Windows command lookup belong to the host. The `process.on("exit")` hook that kills live process groups when
//! the host exits is the host's too: the Node host has it, the native host does not (Rust has no safe process-exit
//! hook), so a native host that exits without closing its transports leaves their servers to notice the closed
//! stdin. Spawn failures and pipe errors are the host's `Error`s with Node's `<syscall> <code>` message
//! (`spawn npx ENOENT`, `write EPIPE`) and `data.code`. A failed write is reported through `onError` by the
//! transport rather than by a stdin `error` event. JSON parse errors carry serde_json's message.

use std::cell::{Cell, RefCell};
use std::rc::{Rc, Weak};

use futures::FutureExt;
use futures::channel::oneshot;
use futures::future::{Either, Shared, select};
use indexmap::IndexMap;
use pi_host::process::{ChildProcess, ProcessListeners, ProcessSignal, SpawnOptions};
use pi_host::{LocalBoxFuture, PiError, PiResult};
use serde_json::Value;

use super::transport::{
	DEFAULT_MAX_MESSAGE_BYTES, McpTransport, McpTransportCloseListener, McpTransportErrorListener,
	McpTransportMessageListener, McpTransportSendOptions, TransportEvents, Unsubscribe,
};
use crate::protocol::jsonrpc::{JsonRpcMessage, McpConnectionClosedError, parse_json_rpc_message};

const DEFAULT_MAX_STDERR_BYTES: usize = 64 * 1024;
const DEFAULT_CLOSE_TIMEOUT_MS: f64 = 2_000.0;
/// How long a server gets to exit on its own after stdin closes, before it is sent SIGTERM.
const STDIN_CLOSE_GRACE_MS: f64 = 500.0;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum StdioStderr {
	#[default]
	Pipe,
	Inherit,
}

/// `onStderr(chunk)`.
pub type StderrListener = Rc<dyn Fn(&str)>;

#[derive(Clone, Default)]
pub struct StdioTransportOptions {
	pub command: String,
	pub args: Option<Vec<String>>,
	pub cwd: Option<String>,
	pub env: Option<IndexMap<String, String>>,
	pub inherit_env: Option<bool>,
	pub stderr: Option<StdioStderr>,
	pub on_stderr: Option<StderrListener>,
	pub max_message_bytes: Option<usize>,
	pub max_stderr_bytes: Option<usize>,
	/// Time to wait for the server to exit after SIGTERM before sending SIGKILL. Default: 2000.
	pub close_timeout_ms: Option<f64>,
}

/// The running server and the `close` event of its process.
struct Child {
	process: Rc<dyn ChildProcess>,
	/// Resolves on the `close` event: the process exited and its stdio streams ended.
	closed: Shared<oneshot::Receiver<()>>,
}

struct Inner {
	events: TransportEvents,
	options: StdioTransportOptions,
	child: RefCell<Option<Rc<Child>>>,
	stdout_buffer: RefCell<Vec<u8>>,
	stderr_buffer: RefCell<Vec<u8>>,
	started: Cell<bool>,
	closed: Cell<bool>,
}

#[derive(Clone)]
pub struct StdioTransport {
	inner: Rc<Inner>,
}

impl StdioTransport {
	/// `new StdioTransport(options)`.
	pub fn new(options: StdioTransportOptions) -> Self {
		Self {
			inner: Rc::new(Inner {
				events: TransportEvents::new(),
				options,
				child: RefCell::new(None),
				stdout_buffer: RefCell::new(Vec::new()),
				stderr_buffer: RefCell::new(Vec::new()),
				started: Cell::new(false),
				closed: Cell::new(false),
			}),
		}
	}

	pub fn options(&self) -> &StdioTransportOptions {
		&self.inner.options
	}

	pub fn pid(&self) -> Option<u32> {
		self.inner.child.borrow().as_ref().and_then(|child| child.process.pid())
	}

	pub fn stderr(&self) -> String {
		String::from_utf8_lossy(&self.inner.stderr_buffer.borrow()).into_owned()
	}
}

impl McpTransport for StdioTransport {
	fn start(&self) -> LocalBoxFuture<'static, PiResult<()>> {
		start(&self.inner)
	}

	fn send(
		&self,
		message: JsonRpcMessage,
		_options: McpTransportSendOptions,
	) -> LocalBoxFuture<'static, PiResult<()>> {
		let inner = self.inner.clone();
		Box::pin(async move {
			let child = inner.child.borrow().clone();
			let Some(child) = child.filter(|child| child.process.stdin_writable()) else {
				return Err(McpConnectionClosedError::new());
			};
			if !inner.started.get() || inner.closed.get() {
				return Err(McpConnectionClosedError::new());
			}
			let payload = format!("{}\n", pi_host::js::json_stringify(&message));
			child.process.write(payload.into()).await.inspect_err(|error| {
				if !inner.closed.get() {
					inner.events.emit_error(error);
				}
			})
		})
	}

	fn close(&self) -> LocalBoxFuture<'static, PiResult<()>> {
		let inner = self.inner.clone();
		Box::pin(async move {
			close(&inner).await;
			Ok(())
		})
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

/// Spawns the server now, like the TS `start` does before its first `await`; the future resolves on `spawn`.
fn start(inner: &Rc<Inner>) -> LocalBoxFuture<'static, PiResult<()>> {
	if inner.started.get() {
		return Box::pin(async { Err(PiError::new("MCP stdio transport already started")) });
	}
	if inner.closed.get() {
		return Box::pin(async { Err(McpConnectionClosedError::new()) });
	}
	inner.started.set(true);
	let options = &inner.options;
	let (close_sender, close_receiver) = oneshot::channel();
	let close_sender = RefCell::new(Some(close_sender));
	// The listeners hold the transport weakly: the host keeps them for the life of the process.
	let weak = || -> Weak<Inner> { Rc::downgrade(inner) };
	let listeners = ProcessListeners {
		on_stdout: Rc::new({
			let inner = weak();
			move |chunk| {
				if let Some(inner) = inner.upgrade() {
					handle_stdout(&inner, chunk);
				}
			}
		}),
		on_stderr: Rc::new({
			let inner = weak();
			move |chunk| {
				if let Some(inner) = inner.upgrade() {
					handle_stderr(&inner, chunk);
				}
			}
		}),
		on_error: Rc::new({
			let inner = weak();
			move |error| {
				if let Some(inner) = inner.upgrade()
					&& !inner.closed.get()
				{
					inner.events.emit_error(&error);
				}
			}
		}),
		on_close: Rc::new({
			let inner = weak();
			move || {
				if let Some(inner) = inner.upgrade() {
					handle_close(&inner);
				}
				if let Some(sender) = close_sender.borrow_mut().take() {
					let _ = sender.send(());
				}
			}
		}),
	};
	let spawn = pi_host::process::spawn_process(
		SpawnOptions {
			command: options.command.clone(),
			args: options.args.clone().unwrap_or_default(),
			cwd: options.cwd.clone(),
			env: options.env.clone().unwrap_or_default(),
			clear_env: options.inherit_env == Some(false),
			inherit_stderr: options.stderr == Some(StdioStderr::Inherit),
		},
		listeners,
	);
	let inner = inner.clone();
	Box::pin(async move {
		let process = spawn.await?;
		*inner.child.borrow_mut() = Some(Rc::new(Child {
			process,
			closed: close_receiver.shared(),
		}));
		Ok(())
	})
}

/// The `close` event: the server is gone, so a partial line left in the buffer is reported and dropped.
fn handle_close(inner: &Inner) {
	*inner.child.borrow_mut() = None;
	let incomplete = !String::from_utf8_lossy(&inner.stdout_buffer.borrow()).trim().is_empty();
	if incomplete {
		inner.events.emit_error(&PiError::new(
			"MCP stdio server closed with an incomplete JSON-RPC message",
		));
	}
	inner.stdout_buffer.borrow_mut().clear();
	inner.events.emit_close();
}

async fn close(inner: &Rc<Inner>) {
	if inner.closed.get() {
		return;
	}
	inner.closed.set(true);
	let child = inner.child.borrow().clone();
	let Some(child) = child else {
		inner.events.emit_close();
		return;
	};
	if child.process.exited() {
		child.process.end_stdin();
		return;
	}
	// Shutdown per the spec: close stdin and let the server exit, then SIGTERM, then SIGKILL.
	let close_timeout_ms = inner.options.close_timeout_ms.unwrap_or(DEFAULT_CLOSE_TIMEOUT_MS);
	let grace = STDIN_CLOSE_GRACE_MS.min(close_timeout_ms);
	child.process.end_stdin();
	let mut closed = child.closed.clone();
	for (delay, signal) in [(grace, ProcessSignal::Term), (close_timeout_ms, ProcessSignal::Kill)] {
		let timer = pi_host::sleep(delay, None);
		futures::pin_mut!(timer);
		match select(closed, timer).await {
			Either::Left(_) => {
				closed = child.closed.clone();
				break;
			}
			Either::Right((_, pending)) => {
				child.process.kill(signal);
				closed = pending;
			}
		}
	}
	let _ = closed.await;
	// Children of the server that ignored stdin closing would otherwise outlive it.
	child.process.kill(ProcessSignal::Term);
}

fn handle_stdout(inner: &Inner, chunk: &[u8]) {
	let max_message_bytes = inner.options.max_message_bytes.unwrap_or(DEFAULT_MAX_MESSAGE_BYTES);
	inner.stdout_buffer.borrow_mut().extend_from_slice(chunk);
	loop {
		let line = {
			let mut buffer = inner.stdout_buffer.borrow_mut();
			match buffer.iter().position(|byte| *byte == b'\n') {
				None => {
					let too_large = buffer.len() > max_message_bytes;
					if too_large {
						buffer.clear();
					}
					drop(buffer);
					if too_large {
						inner.events.emit_error(&message_too_large(max_message_bytes));
					}
					return;
				}
				Some(newline) => {
					let mut line: Vec<u8> = buffer.drain(..=newline).collect();
					line.pop();
					line
				}
			}
		};
		if line.len() > max_message_bytes {
			inner.events.emit_error(&message_too_large(max_message_bytes));
			continue;
		}
		let text = String::from_utf8_lossy(&line);
		let text = text.strip_suffix('\r').unwrap_or(&text);
		if text.trim().is_empty() {
			continue;
		}
		let message = serde_json::from_str::<Value>(text)
			.map_err(PiError::from)
			.and_then(parse_json_rpc_message);
		match message {
			Ok(message) => inner.events.emit_message(&message),
			Err(error) => inner.events.emit_error(&error),
		}
	}
}

fn message_too_large(max_message_bytes: usize) -> PiError {
	PiError::new(format!("MCP stdio message exceeds {max_message_bytes} bytes"))
}

fn handle_stderr(inner: &Inner, chunk: &[u8]) {
	let max_stderr_bytes = inner.options.max_stderr_bytes.unwrap_or(DEFAULT_MAX_STDERR_BYTES);
	{
		let mut buffer = inner.stderr_buffer.borrow_mut();
		buffer.extend_from_slice(chunk);
		if buffer.len() > max_stderr_bytes {
			let excess = buffer.len() - max_stderr_bytes;
			buffer.drain(..excess);
		}
	}
	if let Some(on_stderr) = &inner.options.on_stderr {
		on_stderr(&String::from_utf8_lossy(chunk));
	}
}
