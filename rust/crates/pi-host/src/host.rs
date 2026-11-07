//! The `Host` trait and the thread-local current host, the counterpart of JavaScript's platform globals.

use std::cell::RefCell;
use std::rc::Rc;

use crate::LocalBoxFuture;
use crate::abort::AbortSignal;
use crate::error::{PiError, PiResult};
use crate::http::{HttpRequest, HttpResponse};
use crate::server::{HttpServer, HttpServerErrorHandler, HttpServerHandler};
use crate::websocket::WebSocket;

/// Platform services. One implementation per embedding: napi (Node), wasm (Workers/browsers), native tokio,
/// and the scripted test host.
pub trait Host {
	/// `globalThis.fetch`. Rejects (returns `Err`) only for network failures and aborts, like fetch.
	fn fetch(&self, request: HttpRequest) -> LocalBoxFuture<'static, PiResult<HttpResponse>>;
	/// `Date.now()` in milliseconds.
	fn now_ms(&self) -> f64;
	/// `performance.now()` in milliseconds, monotonic.
	fn monotonic_ms(&self) -> f64;
	/// `process.env[key]`. Hosts without an environment (browsers) return `None`.
	fn env(&self, key: &str) -> Option<String>;
	/// `new Promise((r) => setTimeout(r, ms))`. Implementations clamp like `setTimeout`: a non-finite delay, or one
	/// below 1 or above 2^31 - 1 ms, waits 1 ms.
	fn sleep(&self, ms: f64) -> LocalBoxFuture<'static, ()>;
	/// Fire-and-forget: the counterpart of `void promise` / an unawaited async call.
	fn spawn_local(&self, future: LocalBoxFuture<'static, ()>);
	/// `fs.existsSync(path)`. Hosts without a file system (browsers, Workers) return `false`.
	fn file_exists(&self, _path: &str) -> bool {
		false
	}
	/// `os.homedir()`. Hosts without a file system (browsers, Workers) return `None`.
	fn home_dir(&self) -> Option<String> {
		None
	}
	/// `new WebSocket(url, { headers })`. Hosts without a WebSocket client return `None`; `Err` is a constructor
	/// throw. See `websocket::connect`.
	fn websocket(
		&self,
		_url: &str,
		_headers: &indexmap::IndexMap<String, String>,
	) -> Option<PiResult<std::rc::Rc<dyn WebSocket>>> {
		None
	}
	/// `fs.promises.readFile(path, "utf-8")`: invalid UTF-8 decodes to U+FFFD and a BOM is kept. Hosts without a file
	/// system (browsers, Workers) fail as for a missing file.
	fn read_file(&self, path: &str) -> LocalBoxFuture<'static, PiResult<String>> {
		let error = file_not_found(path);
		Box::pin(async move { Err(error) })
	}
	/// `child_process.spawn(command, args, options)` with piped stdio. Hosts without processes (browsers, Workers)
	/// fail with `process::processes_unsupported`. See `process::spawn_process`.
	fn spawn_process(
		&self,
		_options: crate::process::SpawnOptions,
		_listeners: crate::process::ProcessListeners,
	) -> LocalBoxFuture<'static, PiResult<Rc<dyn crate::process::ChildProcess>>> {
		Box::pin(async { Err(crate::process::processes_unsupported()) })
	}
	/// `http.createServer(handler).listen(port, host)`, resolving once it listens; see `server`. Hosts without an
	/// HTTP listener reject with `server::listen_unsupported()`.
	fn listen_http(
		&self,
		_host: &str,
		_port: u16,
		_handler: HttpServerHandler,
		_on_error: HttpServerErrorHandler,
	) -> LocalBoxFuture<'static, PiResult<Rc<dyn HttpServer>>> {
		Box::pin(async { Err(crate::server::listen_unsupported()) })
	}
}

/// The error Node's `fs.promises.readFile` rejects with for a missing file.
pub fn file_not_found(path: &str) -> PiError {
	PiError::new(format!("ENOENT: no such file or directory, open '{path}'"))
		.with_data(serde_json::json!({ "errno": -2, "code": "ENOENT", "syscall": "open", "path": path }))
}

thread_local! {
	static HOST: RefCell<Option<Rc<dyn Host>>> = const { RefCell::new(None) };
}

/// Installs the host for the current thread and returns the previous one.
pub fn set_host(host: Rc<dyn Host>) -> Option<Rc<dyn Host>> {
	HOST.with(|slot| slot.replace(Some(host)))
}

/// The host of the current thread. Panics when no host is installed: every embedding installs one at startup.
pub fn current() -> Rc<dyn Host> {
	HOST.with(|slot| slot.borrow().clone())
		.expect("pi-host: no Host installed on this thread; call pi_host::set_host")
}

/// `Date.now()`.
pub fn now_ms() -> f64 {
	current().now_ms()
}

/// `performance.now()`.
pub fn monotonic_ms() -> f64 {
	current().monotonic_ms()
}

/// `process.env[key]`, treating an empty string like JavaScript code that tests truthiness would not: callers
/// that check `if (process.env.X)` must also check `!value.is_empty()`.
pub fn env(key: &str) -> Option<String> {
	current().env(key)
}

/// Sleeps `ms` milliseconds. Resolves early with `Err(abort reason)` when `signal` aborts, like pi-ai's
/// `sleep(ms, signal)` helper.
pub async fn sleep(ms: f64, signal: Option<&AbortSignal>) -> PiResult<()> {
	let host = current();
	match signal {
		None => {
			host.sleep(ms).await;
			Ok(())
		}
		Some(signal) => {
			signal.throw_if_aborted()?;
			let aborted = signal.aborted_future();
			futures::pin_mut!(aborted);
			match futures::future::select(host.sleep(ms), aborted).await {
				futures::future::Either::Left(_) => Ok(()),
				futures::future::Either::Right((reason, _)) => Err(reason),
			}
		}
	}
}

/// `void promise`.
pub fn spawn_local(future: impl std::future::Future<Output = ()> + 'static) {
	current().spawn_local(Box::pin(future));
}

/// `fs.existsSync(path)`; `false` on hosts without a file system.
pub fn file_exists(path: &str) -> bool {
	current().file_exists(path)
}

/// `os.homedir()`; `None` on hosts without a file system.
pub fn home_dir() -> Option<String> {
	current().home_dir()
}

/// `await fs.promises.readFile(path, "utf-8")`; fails as for a missing file on hosts without a file system.
pub async fn read_file(path: &str) -> PiResult<String> {
	current().read_file(path).await
}
