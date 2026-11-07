//! Child processes of the native host: tokio processes. On Unix the child runs in its own process group (like
//! `detached: true`) and signals go through `nix`. On Windows `cross-spawn` is matched by resolving the command
//! through `PATH` and `PATHEXT`; Rust's `Command` then runs `.cmd`/`.bat` shims through `cmd.exe` with its own
//! argument escaping, and the tree is killed with `taskkill /T /F`. Spawn failures and pipe errors are `Error`s
//! with Node's `<syscall> <code>` message (`spawn npx ENOENT`, `write EPIPE`) and `data.code`; codes other than
//! `ENOENT`, `EACCES`, `EPIPE` and `ECONNRESET` become `EIO`.

use std::cell::Cell;
use std::rc::Rc;

use bytes::Bytes;
use serde_json::json;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt};
use tokio::process::{ChildStdin, Command};

use crate::LocalBoxFuture;
use crate::error::{PiError, PiResult};
use crate::process::{ChildProcess, DataListener, ProcessListeners, ProcessSignal, SpawnOptions};

const READ_CHUNK_BYTES: usize = 64 * 1024;

/// `spawn(command, args, options)`: starts the process and the task that reads its output and reports `close`.
pub(crate) fn spawn(options: SpawnOptions, listeners: ProcessListeners) -> PiResult<Rc<dyn ChildProcess>> {
	let mut command = Command::new(resolve_command(&options));
	command.args(&options.args);
	if let Some(cwd) = &options.cwd {
		command.current_dir(cwd);
	}
	if options.clear_env {
		command.env_clear();
	}
	command.envs(&options.env);
	command.stdin(std::process::Stdio::piped());
	command.stdout(std::process::Stdio::piped());
	command.stderr(if options.inherit_stderr {
		std::process::Stdio::inherit()
	} else {
		std::process::Stdio::piped()
	});
	// Own process group, so killing the group also terminates the server's children.
	#[cfg(unix)]
	command.process_group(0);
	#[cfg(windows)]
	command.creation_flags(CREATE_NO_WINDOW);
	let mut process = command
		.spawn()
		.map_err(|error| io_error(&error, &format!("spawn {}", options.command)))?;

	let child = Rc::new(NativeChild {
		pid: process.id(),
		stdin: Rc::new(tokio::sync::Mutex::new(process.stdin.take())),
		stdin_writable: Cell::new(true),
		exited: Rc::new(Cell::new(false)),
	});
	let stdout = process.stdout.take();
	let stderr = process.stderr.take();
	let exited = child.exited.clone();
	tokio::task::spawn_local(async move {
		let wait = async {
			let _ = process.wait().await;
			exited.set(true);
		};
		let read_stdout = read_stream(stdout, &listeners.on_stdout, &listeners.on_error);
		let read_stderr = read_stream(stderr, &listeners.on_stderr, &listeners.on_error);
		futures::join!(wait, read_stdout, read_stderr);
		(listeners.on_close)();
	});
	Ok(child)
}

async fn read_stream(stream: Option<impl AsyncRead + Unpin>, on_data: &DataListener, on_error: &Rc<dyn Fn(PiError)>) {
	let Some(mut stream) = stream else {
		return;
	};
	let mut chunk = vec![0u8; READ_CHUNK_BYTES];
	loop {
		match stream.read(&mut chunk).await {
			Ok(0) => return,
			Ok(read) => on_data(&chunk[..read]),
			Err(error) => {
				on_error(io_error(&error, "read"));
				return;
			}
		}
	}
}

struct NativeChild {
	pid: Option<u32>,
	stdin: Rc<tokio::sync::Mutex<Option<ChildStdin>>>,
	stdin_writable: Cell<bool>,
	exited: Rc<Cell<bool>>,
}

impl ChildProcess for NativeChild {
	fn pid(&self) -> Option<u32> {
		self.pid
	}

	fn write(&self, data: Bytes) -> LocalBoxFuture<'static, PiResult<()>> {
		let stdin = self.stdin.clone();
		Box::pin(async move {
			let mut stdin = stdin.lock().await;
			let Some(writer) = stdin.as_mut() else {
				return Err(PiError::new("write EPIPE").with_data(json!({ "code": "EPIPE" })));
			};
			let written = match writer.write_all(&data).await {
				Ok(()) => writer.flush().await,
				Err(error) => Err(error),
			};
			written.map_err(|error| io_error(&error, "write"))
		})
	}

	fn end_stdin(&self) {
		self.stdin_writable.set(false);
		let stdin = self.stdin.clone();
		tokio::task::spawn_local(async move {
			if let Some(mut stdin) = stdin.lock().await.take() {
				let _ = stdin.shutdown().await;
			}
		});
	}

	fn stdin_writable(&self) -> bool {
		self.stdin_writable.get()
	}

	fn exited(&self) -> bool {
		self.exited.get()
	}

	#[cfg(unix)]
	fn kill(&self, signal: ProcessSignal) {
		use nix::sys::signal::{Signal, kill, killpg};
		use nix::unistd::Pid;

		let Some(pid) = self.pid.and_then(|pid| i32::try_from(pid).ok()) else {
			return;
		};
		let signal = match signal {
			ProcessSignal::Term => Signal::SIGTERM,
			ProcessSignal::Kill => Signal::SIGKILL,
		};
		// The whole group, so wrappers like `npx` or `uvx` do not leave the server behind.
		if killpg(Pid::from_raw(pid), signal).is_ok() {
			return;
		}
		// The group is gone or was never created; fall back to the direct child, unless it already exited.
		if !self.exited.get() {
			let _ = kill(Pid::from_raw(pid), signal);
		}
	}

	#[cfg(windows)]
	fn kill(&self, _signal: ProcessSignal) {
		let Some(pid) = self.pid else {
			return;
		};
		if self.exited.get() {
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
}

#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

#[cfg(not(windows))]
fn resolve_command(options: &SpawnOptions) -> std::path::PathBuf {
	options.command.clone().into()
}

/// `cross-spawn`'s lookup: a bare command is searched on the child's `PATH` with every `PATHEXT` extension,
/// so `npx` finds `npx.cmd`.
#[cfg(windows)]
fn resolve_command(options: &SpawnOptions) -> std::path::PathBuf {
	use std::path::{Path, PathBuf};

	let command = Path::new(&options.command);
	if command.components().count() > 1 || command.extension().is_some() {
		return command.to_path_buf();
	}
	let lookup = |name: &str| -> Option<String> {
		let configured = options
			.env
			.iter()
			.find(|(key, _)| key.eq_ignore_ascii_case(name))
			.map(|(_, value)| value.clone());
		match configured {
			Some(value) => Some(value),
			None if options.clear_env => None,
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

#[cfg(all(test, unix))]
mod tests {
	use std::cell::RefCell;

	use super::*;
	use crate::native::run;
	use crate::process::spawn_process;

	/// Listeners that collect stdout and stderr and resolve `closed` on `close`.
	fn collecting() -> (
		ProcessListeners,
		Rc<RefCell<(Vec<u8>, Vec<u8>)>>,
		futures::channel::oneshot::Receiver<()>,
	) {
		let output = Rc::new(RefCell::new((Vec::new(), Vec::new())));
		let (closed, on_closed) = futures::channel::oneshot::channel();
		let closed = RefCell::new(Some(closed));
		let (stdout, stderr) = (output.clone(), output.clone());
		let listeners = ProcessListeners {
			on_stdout: Rc::new(move |data| stdout.borrow_mut().0.extend_from_slice(data)),
			on_stderr: Rc::new(move |data| stderr.borrow_mut().1.extend_from_slice(data)),
			on_error: Rc::new(|error| panic!("unexpected process error: {error:?}")),
			on_close: Rc::new(move || {
				if let Some(closed) = closed.borrow_mut().take() {
					let _ = closed.send(());
				}
			}),
		};
		(listeners, output, on_closed)
	}

	fn shell(script: &str) -> SpawnOptions {
		SpawnOptions {
			command: "sh".into(),
			args: vec!["-c".into(), script.into()],
			..Default::default()
		}
	}

	#[test]
	fn pipes_stdin_stdout_and_stderr_and_reports_close() {
		run(|| async {
			let (listeners, output, closed) = collecting();
			let child = spawn_process(shell("cat; echo done >&2"), listeners).await.unwrap();
			assert!(child.pid().is_some());
			assert!(child.stdin_writable());
			child.write(Bytes::from_static(b"hello\n")).await.unwrap();
			child.end_stdin();
			closed.await.unwrap();
			assert!(child.exited());
			assert_eq!(*output.borrow(), (b"hello\n".to_vec(), b"done\n".to_vec()));
		});
	}

	#[test]
	fn passes_env_and_cwd_and_clears_the_environment_on_request() {
		run(|| async {
			let (listeners, output, closed) = collecting();
			let mut options = shell("printf '%s|%s|%s' \"$PI_TEST\" \"$HOME\" \"$(pwd)\"");
			options.env.insert("PI_TEST".into(), "set".into());
			options.cwd = Some("/".into());
			options.clear_env = true;
			spawn_process(options, listeners).await.unwrap();
			closed.await.unwrap();
			assert_eq!(output.borrow().0, b"set||/".to_vec());
		});
	}

	#[test]
	fn kill_terminates_the_process_group() {
		run(|| async {
			let (listeners, _output, closed) = collecting();
			let child = spawn_process(shell("sleep 30 & wait"), listeners).await.unwrap();
			child.kill(ProcessSignal::Kill);
			closed.await.unwrap();
			assert!(child.exited());
		});
	}

	#[test]
	fn fails_like_the_error_event_for_a_missing_command() {
		run(|| async {
			let (listeners, _output, _closed) = collecting();
			let options = SpawnOptions {
				command: "pi-host-test-missing-command".into(),
				..Default::default()
			};
			let Err(error) = spawn_process(options, listeners).await else {
				panic!("spawning a missing command succeeded");
			};
			assert_eq!(error.message, "spawn pi-host-test-missing-command ENOENT");
		});
	}
}
