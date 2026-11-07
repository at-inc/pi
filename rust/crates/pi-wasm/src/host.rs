//! `JsHost`: `pi_host::Host` over the JS host object passed to `init` (`PiHost` in `@at-inc/pi-core`). Every call
//! goes to the host object at call time, so JS code that replaces `globalThis.fetch` or installs fake timers behind
//! the host sees the core's calls.

use std::rc::Rc;

use indexmap::IndexMap;
use js_sys::{Function, Object, Promise, Reflect};
use pi_host::process::{ChildProcess, ProcessListeners, SpawnOptions};
use pi_host::server::{HttpServer, HttpServerErrorHandler, HttpServerHandler};
use pi_host::websocket::WebSocket;
use pi_host::{Host, HttpRequest, HttpResponse, LocalBoxFuture, PiResult};
use wasm_bindgen::JsCast;
use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::JsFuture;

#[wasm_bindgen(typescript_custom_section)]
const PI_HOST: &str = r#"
/** The platform services the Rust core calls. `@at-inc/pi-core` ships one for Node and one for Workers. */
export interface PiHost {
	/** `globalThis.fetch`. */
	fetch(url: string, init: RequestInit): Promise<Response>;
	/** `Date.now()`. */
	now(): number;
	/** `performance.now()`. */
	monotonic(): number;
	/** `process.env[name]`; `undefined` without an environment. */
	env(name: string): string | undefined;
	/** Resolves after `ms` milliseconds (`setTimeout`), or at once with the timer cleared when `signal` aborts: the core
	 * aborts it when it stops waiting. */
	sleep(ms: number, signal?: AbortSignal): Promise<void>;
	/** `fs.existsSync(path)`; `false` without a file system. */
	fileExists(path: string): boolean;
	/** `os.homedir()`; `undefined` without a file system. */
	homeDir(): string | undefined;
	/** `fs.promises.readFile(path, "utf-8")`; rejects like a missing file without a file system. */
	readFile(path: string): Promise<string>;
	/** `new WebSocket(url, { headers })`; `undefined` without a WebSocket client. */
	webSocket(url: string, headers: Record<string, string>): WebSocket | undefined;
	/** `"<platform>; <runtime>/<version>; <arch>"` for the detailed `User-Agent`; `undefined` outside Node.js and Bun. */
	runtimeDescription?(): string | undefined;
	/**
	 * `child_process.spawn(command, args, options)` with piped stdin and stdout, in its own process group on Unix
	 * (`detached: true`); resolves on `spawn` and rejects with the `error` of a process that could not start.
	 * `undefined` without processes.
	 */
	spawnProcess(options: PiSpawnOptions, listeners: PiProcessListeners): Promise<PiChildProcess> | undefined;
	/**
	 * `http.createServer(handler).listen(port, host)`: resolves with the server once it listens and rejects with the
	 * listen error. `onError` receives the server's later `error` events.
	 */
	listen(
		host: string,
		port: number,
		handler: (request: PiHttpRequest) => Promise<PiHttpResponse>,
		onError: (error: unknown) => void,
	): Promise<PiHttpServer>;
}

/** The options of `PiHost.spawnProcess`. `env` is set on top of the host's environment unless `clearEnv` is set. */
export interface PiSpawnOptions {
	command: string;
	args: string[];
	cwd?: string;
	env: Record<string, string>;
	clearEnv: boolean;
	/** `stdio[2]`: `"inherit"` instead of `"pipe"`. */
	inheritStderr: boolean;
}

/** What a spawned process reports: stdout and stderr `data`, stream and process errors, and `close` once. */
export interface PiProcessListeners {
	onStdout(chunk: Uint8Array): void;
	onStderr(chunk: Uint8Array): void;
	onError(error: unknown): void;
	onClose(): void;
}

/** A process `PiHost.spawnProcess` started. */
export interface PiChildProcess {
	readonly pid: number | undefined;
	/** `stdin.write(data, callback)`: resolves once written, rejects with the write error. */
	write(data: Uint8Array): Promise<void>;
	/** `stdin.end()`. */
	endStdin(): void;
	/** `stdin.writable`. */
	readonly stdinWritable: boolean;
	/** `exitCode !== null || signalCode !== null`. */
	readonly exited: boolean;
	/** Signals the process and its children: the process group on Unix, `taskkill /T /F` on Windows. */
	kill(signal: "SIGTERM" | "SIGKILL"): void;
}

/** A request of `PiHost.listen`: its method and raw target. */
export interface PiHttpRequest {
	method: string;
	url: string;
}

/** `response.writeHead(status, headers)` plus `response.end(body)`. */
export interface PiHttpResponse {
	status: number;
	headers: Record<string, string>;
	body: string;
}

/** A server of `PiHost.listen`. */
export interface PiHttpServer {
	readonly port: number;
	close(): void;
	closeAllConnections(): void;
}
"#;

#[wasm_bindgen]
extern "C" {
	/// The JS host object.
	#[wasm_bindgen(typescript_type = "PiHost")]
	pub type JsHostObject;
	#[wasm_bindgen(method, getter)]
	fn fetch(this: &JsHostObject) -> Function;
	#[wasm_bindgen(method)]
	fn now(this: &JsHostObject) -> f64;
	#[wasm_bindgen(method)]
	fn monotonic(this: &JsHostObject) -> f64;
	#[wasm_bindgen(method)]
	fn env(this: &JsHostObject, name: &str) -> Option<String>;
	#[wasm_bindgen(method)]
	fn sleep(this: &JsHostObject, ms: f64, signal: &JsValue) -> Promise;
	#[wasm_bindgen(method, js_name = fileExists)]
	fn file_exists(this: &JsHostObject, path: &str) -> bool;
	#[wasm_bindgen(method, js_name = homeDir)]
	fn home_dir(this: &JsHostObject) -> Option<String>;
	#[wasm_bindgen(method, catch, js_name = readFile)]
	fn read_file(this: &JsHostObject, path: &str) -> Result<Promise, JsValue>;
	#[wasm_bindgen(method, catch, js_name = webSocket)]
	fn web_socket(this: &JsHostObject, url: &str, headers: &JsValue) -> Result<JsValue, JsValue>;
	#[wasm_bindgen(method, catch)]
	fn listen(
		this: &JsHostObject,
		host: &str,
		port: u16,
		handler: &JsValue,
		on_error: &JsValue,
	) -> Result<Promise, JsValue>;
	#[wasm_bindgen(method, catch, js_name = spawnProcess)]
	fn spawn_process(this: &JsHostObject, options: &JsValue, listeners: &JsValue) -> Result<JsValue, JsValue>;

	#[wasm_bindgen(js_namespace = console, js_name = error)]
	fn console_error(message: &str);
}

/// Installs `host` as the core's platform. Call once per wasm instance before any other export; a panic is reported
/// with `console.error` before the wasm traps.
#[wasm_bindgen]
pub fn init(host: JsHostObject) {
	std::panic::set_hook(Box::new(|info| console_error(&info.to_string())));
	pi_host::set_host(Rc::new(JsHost { object: host }));
}

struct JsHost {
	object: JsHostObject,
}

impl Host for JsHost {
	fn fetch(&self, request: HttpRequest) -> LocalBoxFuture<'static, PiResult<HttpResponse>> {
		crate::fetch::js_fetch(self.object.fetch(), JsValue::clone(&self.object), request)
	}

	fn now_ms(&self) -> f64 {
		self.object.now()
	}

	fn monotonic_ms(&self) -> f64 {
		self.object.monotonic()
	}

	fn env(&self, key: &str) -> Option<String> {
		self.object.env(key)
	}

	fn sleep(&self, ms: f64) -> LocalBoxFuture<'static, ()> {
		let ms = if ms.is_finite() && (1.0..=2_147_483_647.0).contains(&ms) {
			ms
		} else {
			1.0
		};
		// The host's timer is cleared when the core drops the wait (an abort won the race), as the TS clears its
		// `setTimeout`, so no timer outlives the wait.
		let cancel = CancelOnDrop(Some(pi_host::AbortController::new()));
		let signal = crate::abort::to_js(&cancel.0.as_ref().expect("set above").signal());
		let timer = JsFuture::from(self.object.sleep(ms, &signal.js));
		Box::pin(async move {
			let mut cancel = cancel;
			let _ = timer.await;
			cancel.0 = None;
			drop(signal);
		})
	}

	fn spawn_local(&self, future: LocalBoxFuture<'static, ()>) {
		wasm_bindgen_futures::spawn_local(future);
	}

	fn file_exists(&self, path: &str) -> bool {
		self.object.file_exists(path)
	}

	fn home_dir(&self) -> Option<String> {
		self.object.home_dir()
	}

	fn read_file(&self, path: &str) -> LocalBoxFuture<'static, PiResult<String>> {
		let read = self.object.read_file(path);
		Box::pin(async move {
			let promise = read.map_err(crate::error::from_js)?;
			let text = JsFuture::from(promise).await.map_err(crate::error::from_js)?;
			Ok(text.as_string().unwrap_or_default())
		})
	}

	fn websocket(&self, url: &str, headers: &IndexMap<String, String>) -> Option<PiResult<Rc<dyn WebSocket>>> {
		let object = Object::new();
		for (name, value) in headers {
			let _ = Reflect::set(&object, &name.as_str().into(), &value.as_str().into());
		}
		match self.object.web_socket(url, &object) {
			Err(error) => Some(Err(crate::error::from_js(error))),
			Ok(socket) if socket.is_undefined() || socket.is_null() => None,
			Ok(socket) => Some(Ok(crate::websocket::JsWebSocket::open(socket))),
		}
	}

	fn spawn_process(
		&self,
		options: SpawnOptions,
		listeners: ProcessListeners,
	) -> LocalBoxFuture<'static, PiResult<Rc<dyn ChildProcess>>> {
		let spawned = self.object.spawn_process(
			&crate::process::options_to_js(&options),
			&crate::process::listeners_to_js(listeners),
		);
		crate::process::spawned(spawned)
	}

	fn listen_http(
		&self,
		host: &str,
		port: u16,
		handler: HttpServerHandler,
		on_error: HttpServerErrorHandler,
	) -> LocalBoxFuture<'static, PiResult<Rc<dyn HttpServer>>> {
		let (handler, on_error) = crate::server::callbacks(handler, on_error);
		crate::server::listening(self.object.listen(host, port, &handler, &on_error))
	}

	fn runtime_description(&self) -> Option<String> {
		let method = Reflect::get(&self.object, &"runtimeDescription".into()).ok()?;
		let method = method.dyn_ref::<Function>()?;
		method.call0(&self.object).ok()?.as_string()
	}
}

/// Aborts its controller when dropped while still set.
struct CancelOnDrop(Option<pi_host::AbortController>);

impl Drop for CancelOnDrop {
	fn drop(&mut self) {
		if let Some(controller) = self.0.take() {
			controller.abort();
		}
	}
}
