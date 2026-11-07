//! Port of `packages/mcp/src/transports/stdio.ts`.
//! PORT-STATUS: reviewed
//!
//! Behind the `native` feature, on tokio processes. On Unix the server runs in its own process group (like
//! `detached: true`) and signals go through `nix`. On Windows `cross-spawn` is matched by resolving the
//! command through `PATH` and `PATHEXT`; Rust's `Command` then runs `.cmd`/`.bat` shims through `cmd.exe` with
//! its own argument escaping, and the tree is killed with `taskkill /T /F`. The `process.on("exit")` hook that
//! kills live process groups when the host exits is not ported: Rust has no safe process-exit hook, so a host
//! that exits without closing its transports leaves their servers to notice the closed stdin. Spawn failures
//! and pipe errors are `Error`s with Node's `<syscall> <code>` message (`spawn npx ENOENT`, `write EPIPE`) and
//! `data.code`; codes other than `ENOENT`, `EACCES`, `EPIPE` and `ECONNRESET` become `EIO`. JSON parse errors carry
//! serde_json's message.

#![cfg(feature = "native")]

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use futures::FutureExt;
use futures::channel::oneshot;
use futures::future::{Either, Shared, select};
use indexmap::IndexMap;
use pi_host::{LocalBoxFuture, PiError, PiResult};
use serde_json::{Value, json};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt};
use tokio::process::{ChildStdin, Command};

use super::transport::{
	DEFAULT_MAX_MESSAGE_BYTES, McpTransport, McpTransportCloseListener, McpTransportErrorListener,
	McpTransportMessageListener, McpTransportSendOptions, TransportEvents, Unsubscribe,
};
use crate::protocol::jsonrpc::{JsonRpcMessage, McpConnectionClosedError, parse_json_rpc_message};

const DEFAULT_MAX_STDERR_BYTES: usize = 64 * 1024;
const DEFAULT_CLOSE_TIMEOUT_MS: f64 = 2_000.0;
/// How long a server gets to exit on its own after stdin closes, before it is sent SIGTERM.
const STDIN_CLOSE_GRACE_MS: f64 = 500.0;
const READ_CHUNK_BYTES: usize = 64 * 1024;

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

#[derive(Clone, Copy)]
enum KillSignal {
	Term,
	Kill,
}

/// The running server: what `ChildProcess` exposes to the transport.
struct Child {
	pid: Option<u32>,
	stdin: Rc<tokio::sync::Mutex<Option<ChildStdin>>>,
	stdin_writable: Cell<bool>,
	/// `exitCode !== null || signalCode !== null`.
	exited: Cell<bool>,
	/// Resolves on the `close` event: the process exited and its stdio streams ended.
	closed: Shared<oneshot::Receiver<()>>,
}

impl Child {
	/// `stdin.end()`: pending writes finish first, then the pipe closes.
	fn end_stdin(&self) {
		self.stdin_writable.set(false);
		let stdin = self.stdin.clone();
		pi_host::spawn_local(async move {
			if let Some(mut stdin) = stdin.lock().await.take() {
				let _ = stdin.shutdown().await;
			}
		});
	}
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
		self.inner.child.borrow().as_ref().and_then(|child| child.pid)
	}

	pub fn stderr(&self) -> String {
		String::from_utf8_lossy(&self.inner.stderr_buffer.borrow()).into_owned()
	}
}

impl McpTransport for StdioTransport {
	fn start(&self) -> LocalBoxFuture<'static, PiResult<()>> {
		let inner = self.inner.clone();
		Box::pin(async move { start(&inner) })
	}

	fn send(
		&self,
		message: JsonRpcMessage,
		_options: McpTransportSendOptions,
	) -> LocalBoxFuture<'static, PiResult<()>> {
		let inner = self.inner.clone();
		Box::pin(async move {
			let child = inner.child.borrow().clone();
			let Some(child) = child.filter(|child| child.stdin_writable.get()) else {
				return Err(McpConnectionClosedError::new());
			};
			if !inner.started.get() || inner.closed.get() {
				return Err(McpConnectionClosedError::new());
			}
			let payload = format!("{}\n", pi_host::js::json_stringify(&message));
			let mut stdin = child.stdin.lock().await;
			let Some(writer) = stdin.as_mut() else {
				return Err(McpConnectionClosedError::new());
			};
			let written = match writer.write_all(payload.as_bytes()).await {
				Ok(()) => writer.flush().await,
				Err(error) => Err(error),
			};
			drop(stdin);
			written.map_err(|error| {
				let error = io_error(&error, "write");
				if !inner.closed.get() {
					inner.events.emit_error(&error);
				}
				error
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

fn start(inner: &Rc<Inner>) -> PiResult<()> {
	if inner.started.get() {
		return Err(PiError::new("MCP stdio transport already started"));
	}
	if inner.closed.get() {
		return Err(McpConnectionClosedError::new());
	}
	inner.started.set(true);
	let options = &inner.options;
	let mut command = Command::new(resolve_command(options));
	command.args(options.args.iter().flatten());
	if let Some(cwd) = &options.cwd {
		command.current_dir(cwd);
	}
	if options.inherit_env == Some(false) {
		command.env_clear();
	}
	command.envs(options.env.iter().flatten());
	command.stdin(std::process::Stdio::piped());
	command.stdout(std::process::Stdio::piped());
	command.stderr(if options.stderr == Some(StdioStderr::Inherit) {
		std::process::Stdio::inherit()
	} else {
		std::process::Stdio::piped()
	});
	// Own process group, so closing the transport can terminate the server's children too.
	#[cfg(unix)]
	command.process_group(0);
	#[cfg(windows)]
	command.creation_flags(CREATE_NO_WINDOW);
	let mut process = command
		.spawn()
		.map_err(|error| io_error(&error, &format!("spawn {}", options.command)))?;

	let (close_sender, close_receiver) = oneshot::channel();
	let child = Rc::new(Child {
		pid: process.id(),
		stdin: Rc::new(tokio::sync::Mutex::new(process.stdin.take())),
		stdin_writable: Cell::new(true),
		exited: Cell::new(false),
		closed: close_receiver.shared(),
	});
	*inner.child.borrow_mut() = Some(child.clone());

	let stdout = process.stdout.take();
	let stderr = process.stderr.take();
	let task_inner = inner.clone();
	pi_host::spawn_local(async move {
		let inner = task_inner;
		let wait = async {
			let _ = process.wait().await;
			child.exited.set(true);
		};
		let read_stdout = read_stream(&inner, stdout, handle_stdout);
		let read_stderr = read_stream(&inner, stderr, handle_stderr);
		futures::join!(wait, read_stdout, read_stderr);
		*inner.child.borrow_mut() = None;
		let incomplete = !String::from_utf8_lossy(&inner.stdout_buffer.borrow()).trim().is_empty();
		if incomplete {
			inner.events.emit_error(&PiError::new(
				"MCP stdio server closed with an incomplete JSON-RPC message",
			));
		}
		inner.stdout_buffer.borrow_mut().clear();
		inner.events.emit_close();
		let _ = close_sender.send(());
	});
	Ok(())
}

async fn read_stream(inner: &Rc<Inner>, stream: Option<impl AsyncRead + Unpin>, handle: fn(&Inner, &[u8])) {
	let Some(mut stream) = stream else {
		return;
	};
	let mut chunk = vec![0u8; READ_CHUNK_BYTES];
	loop {
		match stream.read(&mut chunk).await {
			Ok(0) => return,
			Ok(read) => handle(inner, &chunk[..read]),
			Err(error) => {
				inner.events.emit_error(&io_error(&error, "read"));
				return;
			}
		}
	}
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
	if child.exited.get() {
		child.end_stdin();
		return;
	}
	// Shutdown per the spec: close stdin and let the server exit, then SIGTERM, then SIGKILL.
	let close_timeout_ms = inner.options.close_timeout_ms.unwrap_or(DEFAULT_CLOSE_TIMEOUT_MS);
	let grace = STDIN_CLOSE_GRACE_MS.min(close_timeout_ms);
	child.end_stdin();
	let mut closed = child.closed.clone();
	for (delay, signal) in [(grace, KillSignal::Term), (close_timeout_ms, KillSignal::Kill)] {
		let timer = pi_host::sleep(delay, None);
		futures::pin_mut!(timer);
		match select(closed, timer).await {
			Either::Left(_) => {
				closed = child.closed.clone();
				break;
			}
			Either::Right((_, pending)) => {
				kill_process_tree(&child, signal);
				closed = pending;
			}
		}
	}
	let _ = closed.await;
	// Children of the server that ignored stdin closing would otherwise outlive it.
	kill_process_tree(&child, KillSignal::Term);
}

#[cfg(unix)]
fn kill_process_tree(child: &Child, signal: KillSignal) {
	use nix::sys::signal::{Signal, kill, killpg};
	use nix::unistd::Pid;

	let Some(pid) = child.pid.and_then(|pid| i32::try_from(pid).ok()) else {
		return;
	};
	let signal = match signal {
		KillSignal::Term => Signal::SIGTERM,
		KillSignal::Kill => Signal::SIGKILL,
	};
	// The whole group, so wrappers like `npx` or `uvx` do not leave the server behind.
	if killpg(Pid::from_raw(pid), signal).is_ok() {
		return;
	}
	// The group is gone or was never created; fall back to the direct child, unless it already exited.
	if !child.exited.get() {
		let _ = kill(Pid::from_raw(pid), signal);
	}
}

#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

#[cfg(windows)]
fn kill_process_tree(child: &Child, _signal: KillSignal) {
	let Some(pid) = child.pid else {
		return;
	};
	if child.exited.get() {
		return;
	}
	// Windows has no graceful signals, and `.cmd` shims run through cmd.exe; killing only that would leave
	// the server running.
	let _ = Command::new("taskkill")
		.args(["/pid", &pid.to_string(), "/T", "/F"])
		.stdin(std::process::Stdio::null())
		.stdout(std::process::Stdio::null())
		.stderr(std::process::Stdio::null())
		.creation_flags(CREATE_NO_WINDOW)
		.spawn();
}

#[cfg(not(windows))]
fn resolve_command(options: &StdioTransportOptions) -> std::path::PathBuf {
	options.command.clone().into()
}

/// `cross-spawn`'s lookup: a bare command is searched on the child's `PATH` with every `PATHEXT` extension,
/// so `npx` finds `npx.cmd`.
#[cfg(windows)]
fn resolve_command(options: &StdioTransportOptions) -> std::path::PathBuf {
	use std::path::{Path, PathBuf};

	let command = Path::new(&options.command);
	if command.components().count() > 1 || command.extension().is_some() {
		return command.to_path_buf();
	}
	let lookup = |name: &str| -> Option<String> {
		let configured = options
			.env
			.iter()
			.flatten()
			.find(|(key, _)| key.eq_ignore_ascii_case(name))
			.map(|(_, value)| value.clone());
		match configured {
			Some(value) => Some(value),
			None if options.inherit_env == Some(false) => None,
			None => std::env::var(name).ok(),
		}
	};
	let extensions = lookup("PATHEXT").unwrap_or_else(|| ".COM;.EXE;.BAT;.CMD".into());
	let mut directories: Vec<PathBuf> = options.cwd.iter().map(PathBuf::from).collect();
	if let Some(path) = lookup("PATH") {
		directories.extend(std::env::split_paths(&path));
	}
	for directory in directories {
		for extension in extensions.split(';').filter(|extension| !extension.is_empty()) {
			let candidate = directory.join(format!("{}{}", options.command, extension));
			if candidate.is_file() {
				return candidate;
			}
		}
	}
	command.to_path_buf()
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

/// A Node system error: the `<syscall> <code>` message with `code` in `data`.
fn io_error(error: &std::io::Error, syscall: &str) -> PiError {
	let code = match error.kind() {
		std::io::ErrorKind::NotFound => "ENOENT",
		std::io::ErrorKind::PermissionDenied => "EACCES",
		std::io::ErrorKind::BrokenPipe => "EPIPE",
		std::io::ErrorKind::ConnectionReset => "ECONNRESET",
		_ => "EIO",
	};
	PiError::new(format!("{syscall} {code}")).with_data(json!({ "code": code }))
}
