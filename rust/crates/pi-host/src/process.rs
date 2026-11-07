//! Child processes: the counterpart of `child_process.spawn` with piped stdio, as the MCP stdio transport uses it
//! (through `cross-spawn` in the TS). Hosts without processes (browsers, Workers) fail `spawn_process`.

use std::rc::Rc;

use bytes::Bytes;
use indexmap::IndexMap;

use crate::LocalBoxFuture;
use crate::error::{PiError, PiResult};

/// The `spawn(command, args, options)` call: stdin and stdout are pipes, the process runs in its own process
/// group on Unix (`detached: true`), and on Windows a bare command is looked up through `PATH` and `PATHEXT`
/// like `cross-spawn` does.
#[derive(Clone, Debug, Default)]
pub struct SpawnOptions {
	pub command: String,
	pub args: Vec<String>,
	pub cwd: Option<String>,
	/// Variables set on top of the host's environment, or the whole environment when `clear_env` is set.
	pub env: IndexMap<String, String>,
	/// Start from an empty environment instead of the host's (`inheritEnv: false`).
	pub clear_env: bool,
	/// `stdio[2]`: `"inherit"` instead of `"pipe"`.
	pub inherit_stderr: bool,
}

/// A `data` listener of stdout or stderr.
pub type DataListener = Rc<dyn Fn(&[u8])>;

/// What a spawned process reports: the `data` events of stdout and stderr, errors of its stdio streams and of the
/// process after it spawned, and `close` (the process exited and its stdio streams ended), which fires once.
#[derive(Clone)]
pub struct ProcessListeners {
	pub on_stdout: DataListener,
	pub on_stderr: DataListener,
	pub on_error: Rc<dyn Fn(PiError)>,
	pub on_close: Rc<dyn Fn()>,
}

/// The signal `kill` sends.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProcessSignal {
	Term,
	Kill,
}

/// A running child process, the `ChildProcess` of Node.
pub trait ChildProcess {
	/// `child.pid`.
	fn pid(&self) -> Option<u32>;
	/// `stdin.write(data, callback)`: resolves once the data is written, fails like the write callback.
	fn write(&self, data: Bytes) -> LocalBoxFuture<'static, PiResult<()>>;
	/// `stdin.end()`: pending writes finish first, then the pipe closes.
	fn end_stdin(&self);
	/// `stdin.writable`.
	fn stdin_writable(&self) -> bool;
	/// `exitCode !== null || signalCode !== null`.
	fn exited(&self) -> bool;
	/// Sends `signal` to the process and its children: the process group on Unix, falling back to the process
	/// itself while it runs, and `taskkill /T /F` on Windows, which has no graceful signals.
	fn kill(&self, signal: ProcessSignal);
}

/// The error of `spawn_process` on hosts without processes.
pub fn processes_unsupported() -> PiError {
	PiError::new("Child processes are not supported on this host")
		.with_data(serde_json::json!({ "code": "ERR_METHOD_NOT_IMPLEMENTED" }))
}

/// Spawns a child process on the current host. Resolves on the `spawn` event and fails like the `error` event of a
/// process that could not start (`spawn npx ENOENT`).
pub fn spawn_process(
	options: SpawnOptions,
	listeners: ProcessListeners,
) -> LocalBoxFuture<'static, PiResult<Rc<dyn ChildProcess>>> {
	crate::host::current().spawn_process(options, listeners)
}
