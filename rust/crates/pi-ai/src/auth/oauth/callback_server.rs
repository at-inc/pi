//! Port of `packages/ai/src/auth/oauth/callback-server.ts`.
//! PORT-STATUS: reviewed
//!
//! Loopback OAuth redirect handler shared by the browser sign-in flows.
//!
//! The listening socket needs the `native` feature: it is a tokio `TcpListener` instead of `node:http`. Without
//! `native`, `startOAuthCallbackServer` rejects with `OAuth callback server requires the native feature`, as a
//! platform that cannot listen would; flows that catch the rejection continue with manual input.
//!
//! Like Node, the server binds the first address the host name resolves to, closes its socket inside `close()`,
//! keeps listening after an accept error (an aborted connection or descriptor exhaustion is retried silently,
//! other errors settle the wait as Node's `error` event does), and keeps the wait alive while its callbacks can
//! still settle it, so dropping the server handle neither rejects `wait()` nor stops the server. Bind errors carry
//! Node's message (`listen EADDRINUSE: address already in use 127.0.0.1:1455`, `getaddrinfo ENOTFOUND host`) and
//! `data` (`errno`, `code`, `syscall`, `address`, `port`); an error kind Node names but Rust does not is `UNKNOWN`.
//!
//! The HTTP side reads one request head per connection, within Node's `headersTimeout` of 60 seconds. It answers a
//! malformed request line (unknown or lowercase method, a target byte outside visible ASCII, bare LF line endings)
//! with a bare 400 and a head over Node's 16 KiB limit with a bare 431, as Node's parser does. The response
//! differs from Node's in bytes: `content-length` instead of chunked encoding, `connection: close` instead of
//! keep-alive, no `Date` header, and a request body is never read. A HEAD request gets the headers only. A request
//! target that is not a valid URL gets the 404 page, where the TS handler throws inside `void (async () => …)()`
//! and the unhandled rejection ends a Node process. The wait promise is a `Shared` future, so `T` is `Clone`. TS
//! returns the anonymous `{ type: "callback", value } | { type: "manual", input }` union of
//! `waitForCallbackOrManualInput` as `CallbackOrManualInput` and takes its prompt as `ManualInputPrompt`; its
//! `finally` also runs when the returned future is dropped.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use futures::FutureExt;
use futures::channel::oneshot;
use futures::future::{AbortHandle, Shared};
use pi_host::abort::ListenerId;
use pi_host::{AbortController, AbortSignal, LocalBoxFuture, PiError, PiResult};

use crate::auth::types::{AuthPrompt, AuthPromptKind, ProviderAuthInteraction};

/// `OAuthCallbackServerOptions.complete`.
pub type OAuthCallbackCompleteFunction<T> = Rc<dyn Fn(String) -> LocalBoxFuture<'static, PiResult<T>>>;

pub struct OAuthCallbackServerOptions<T> {
	/// Provider name used on the browser page, for example `OpenAI`.
	pub provider_name: String,
	/// Address to listen on.
	pub host: String,
	/// Port to listen on; `0` picks a free port.
	pub port: u16,
	pub path: String,
	/// Host in `redirectUri` when it differs from `host`, for example `localhost`.
	pub redirect_host: Option<String>,
	/// Expected `state` parameter. Omit when the provider does not send one.
	pub state: Option<String>,
	/// Finishes the sign-in with the received code before the browser page is sent, so the page can
	/// show exchange failures. Pass `async (code) => code` to exchange the code later.
	pub complete: OAuthCallbackCompleteFunction<T>,
	pub signal: Option<AbortSignal>,
	pub timeout_ms: Option<f64>,
}

type WaitResult<T> = Result<Option<T>, PiError>;

struct CallbackState<T> {
	claimed: Cell<bool>,
	settled: Cell<bool>,
	sender: RefCell<Option<oneshot::Sender<WaitResult<T>>>>,
	timer: RefCell<Option<AbortHandle>>,
	abort_listener: RefCell<Option<(AbortSignal, ListenerId)>>,
	/// Closes the listening socket at once, like Node's `server.close()`.
	close_listener: RefCell<Option<Box<dyn FnOnce()>>>,
}

impl<T> CallbackState<T> {
	fn finish(&self, result: WaitResult<T>) {
		if self.settled.get() {
			return;
		}
		self.settled.set(true);
		if let Some(timer) = self.timer.borrow_mut().take() {
			timer.abort();
		}
		if let Some((signal, listener)) = self.abort_listener.borrow_mut().take() {
			signal.remove_listener(listener);
		}
		if let Some(sender) = self.sender.borrow_mut().take() {
			let _ = sender.send(result);
		}
	}
}

#[derive(Clone)]
pub struct OAuthCallbackServer<T: Clone + 'static> {
	pub redirect_uri: String,
	state: Rc<CallbackState<T>>,
	wait: Shared<LocalBoxFuture<'static, WaitResult<T>>>,
}

impl<T: Clone + 'static> OAuthCallbackServer<T> {
	/// Resolves with the result of `complete`, or `undefined` after `cancel()`. Rejects when the provider
	/// redirects with an error, `complete` fails, the signal aborts, or the timeout elapses.
	pub fn wait(&self) -> LocalBoxFuture<'static, WaitResult<T>> {
		Box::pin(self.wait.clone())
	}

	/// Stop waiting for the browser unless a callback is already being completed.
	pub fn cancel(&self) {
		if !self.state.claimed.get() {
			self.state.finish(Ok(None));
		}
	}

	pub fn close(&self) {
		self.state.finish(Err(PiError::new("OAuth callback server closed")));
		let close_listener = self.state.close_listener.borrow_mut().take();
		if let Some(close_listener) = close_listener {
			close_listener();
		}
	}
}

pub async fn start_oauth_callback_server<T: Clone + 'static>(
	options: OAuthCallbackServerOptions<T>,
) -> PiResult<OAuthCallbackServer<T>> {
	if options.signal.as_ref().is_some_and(AbortSignal::aborted) {
		return Err(PiError::new("Login cancelled"));
	}

	let (sender, receiver) = oneshot::channel::<WaitResult<T>>();
	// A cancelled or closed wait may never be observed.
	let wait = receiver
		.map(|result| result.unwrap_or_else(|_| Err(PiError::new("OAuth callback server closed"))))
		.boxed_local()
		.shared();
	let state = Rc::new(CallbackState {
		claimed: Cell::new(false),
		settled: Cell::new(false),
		sender: RefCell::new(Some(sender)),
		timer: RefCell::new(None),
		abort_listener: RefCell::new(None),
		close_listener: RefCell::new(None),
	});

	let port = native::listen(&options, state.clone()).await?;

	if let Some(signal) = &options.signal {
		let aborted = state.clone();
		let listener = signal.add_listener(move |_| aborted.finish(Err(PiError::new("Login cancelled"))));
		*state.abort_listener.borrow_mut() = Some((signal.clone(), listener));
	}
	if let Some(timeout_ms) = options.timeout_ms {
		let (timer, registration) = AbortHandle::new_pair();
		*state.timer.borrow_mut() = Some(timer);
		let expired = state.clone();
		let message = format!("{} sign-in timed out", options.provider_name);
		// Created here so the timer starts now, as `setTimeout` does.
		let sleep = pi_host::current().sleep(timeout_ms);
		pi_host::spawn_local(async move {
			if futures::future::Abortable::new(sleep, registration).await.is_ok() {
				expired.finish(Err(PiError::new(message)));
			}
		});
	}
	let redirect_host = options.redirect_host.as_deref().unwrap_or(&options.host);
	let redirect_host = if redirect_host.contains(':') {
		format!("[{redirect_host}]")
	} else {
		redirect_host.to_string()
	};
	Ok(OAuthCallbackServer {
		redirect_uri: format!("http://{redirect_host}:{port}{}", options.path),
		state,
		wait,
	})
}

#[cfg(not(feature = "native"))]
mod native {
	use std::rc::Rc;

	use pi_host::{PiError, PiResult};

	use super::{CallbackState, OAuthCallbackServerOptions};

	pub(super) async fn listen<T>(
		_options: &OAuthCallbackServerOptions<T>,
		_state: Rc<CallbackState<T>>,
	) -> PiResult<u16> {
		Err(PiError::new("OAuth callback server requires the native feature"))
	}
}

#[cfg(feature = "native")]
mod native {
	use std::rc::Rc;

	use pi_host::{PiError, PiResult};
	use url::Url;

	use super::loopback::{LoopbackRequest, LoopbackResponse};
	use super::{CallbackState, OAuthCallbackServerOptions};
	use crate::utils::oauth_page::{oauth_error_html, oauth_success_html};

	/// Binds the socket and starts accepting; returns the bound port.
	pub(super) async fn listen<T: 'static>(
		options: &OAuthCallbackServerOptions<T>,
		state: Rc<CallbackState<T>>,
	) -> PiResult<u16> {
		let provider_name = options.provider_name.clone();
		let path = options.path.clone();
		let expected_state = options.state.clone();
		let complete = options.complete.clone();
		let callback = state.clone();
		let handler = Rc::new(move |request: LoopbackRequest| {
			let provider_name = provider_name.clone();
			let path = path.clone();
			let expected_state = expected_state.clone();
			let complete = complete.clone();
			let callback = callback.clone();
			Box::pin(async move {
				let url = Url::parse("http://localhost").and_then(|base| base.join(&request.target));
				let url = match url {
					Ok(url) if request.method == "GET" && url.path() == path => url,
					_ => return page(404, oauth_error_html("Callback route not found.", None)),
				};
				let param = |name: &str| {
					url.query_pairs()
						.find(|(key, _)| key == name)
						.map(|(_, value)| value.into_owned())
				};
				if let Some(expected) = &expected_state
					&& param("state").as_ref() != Some(expected)
				{
					return page(400, oauth_error_html("State mismatch.", None));
				}
				if callback.claimed.get() || callback.settled.get() {
					return page(409, oauth_error_html("This sign-in has already been handled.", None));
				}
				// TS sends each page and settles the wait in the same tick, so the wait settles before the page
				// is written.
				if let Some(error) = param("error").filter(|error| !error.is_empty()) {
					let description = param("error_description").unwrap_or(error);
					callback.finish(Err(PiError::new(format!(
						"{provider_name} authorization failed: {description}"
					))));
					return page(
						400,
						oauth_error_html(&format!("{provider_name} authorization failed."), Some(&description)),
					);
				}
				let Some(code) = param("code").filter(|code| !code.is_empty()) else {
					return page(400, oauth_error_html("Missing authorization code.", None));
				};
				callback.claimed.set(true);
				match complete(code).await {
					Ok(value) => {
						callback.finish(Ok(Some(value)));
						page(
							200,
							oauth_success_html(&format!("Signed in to {provider_name}. You may now close this page.")),
						)
					}
					Err(failure) => {
						let html =
							oauth_error_html(&format!("{provider_name} sign-in failed."), Some(&failure.message));
						callback.finish(Err(failure));
						page(502, html)
					}
				}
			}) as pi_host::LocalBoxFuture<'static, LoopbackResponse>
		});
		let on_error = {
			let state = state.clone();
			Rc::new(move |error: PiError| state.finish(Err(error)))
		};
		let server = super::loopback::listen(&options.host, options.port, handler, on_error).await?;
		let port = server.port;
		*state.close_listener.borrow_mut() = Some(Box::new(move || server.close()));
		Ok(port)
	}

	fn page(status: u16, html: String) -> LoopbackResponse {
		LoopbackResponse {
			status,
			headers: vec![
				("content-type", "text/html; charset=utf-8"),
				("cache-control", "no-store"),
			],
			body: html,
		}
	}
}

/// The subset of `node:http` the loopback OAuth servers use: one request head per connection, answered by a
/// handler, with Node's binding, parser rejections and `close()` behaviour.
#[cfg(feature = "native")]
pub(crate) mod loopback {
	use std::cell::{Cell, RefCell};
	use std::collections::HashMap;
	use std::future::poll_fn;
	use std::io::ErrorKind;
	use std::net::SocketAddr;
	use std::rc::Rc;
	use std::task::Poll;

	use futures::future::{AbortHandle, Abortable, Either, select};
	use pi_host::{LocalBoxFuture, PiError, PiResult};
	use serde_json::json;
	use tokio::io::{AsyncReadExt, AsyncWriteExt};
	use tokio::net::{TcpListener, TcpStream};

	/// Method and raw target of a request.
	pub(crate) struct LoopbackRequest {
		pub method: String,
		pub target: String,
	}

	/// `response.writeHead(status, headers)` plus `response.end(body)`.
	pub(crate) struct LoopbackResponse {
		pub status: u16,
		pub headers: Vec<(&'static str, &'static str)>,
		pub body: String,
	}

	pub(crate) type LoopbackHandler = Rc<dyn Fn(LoopbackRequest) -> LocalBoxFuture<'static, LoopbackResponse>>;
	/// The server's `error` event after it listens.
	pub(crate) type LoopbackErrorHandler = Rc<dyn Fn(PiError)>;

	/// A listening server; dropping it leaves it listening, like an unreferenced Node server.
	pub(crate) struct LoopbackServer {
		pub port: u16,
		socket: Rc<RefCell<Option<TcpListener>>>,
		accepting: AbortHandle,
		connections: Rc<RefCell<HashMap<u64, AbortHandle>>>,
	}

	impl LoopbackServer {
		/// `server.close()`: closes the listening socket now; open connections finish their response.
		pub(crate) fn close(&self) {
			self.socket.borrow_mut().take();
			self.accepting.abort();
		}

		/// `server.closeAllConnections()`.
		pub(crate) fn close_all_connections(&self) {
			for (_, connection) in self.connections.borrow_mut().drain() {
				connection.abort();
			}
		}
	}

	/// `createServer(handler).listen(port, host)`: binds the first address the host resolves to.
	pub(crate) async fn listen(
		host: &str,
		port: u16,
		handler: LoopbackHandler,
		on_error: LoopbackErrorHandler,
	) -> PiResult<LoopbackServer> {
		let address = tokio::net::lookup_host((host, port))
			.await
			.ok()
			.and_then(|mut addresses| addresses.next());
		let Some(address) = address else {
			return Err(PiError::new(format!("getaddrinfo ENOTFOUND {host}")).with_data(json!({
				"errno": -3008,
				"code": "ENOTFOUND",
				"syscall": "getaddrinfo",
				"hostname": host,
			})));
		};
		let listener = TcpListener::bind(address)
			.await
			.map_err(|error| listen_error(&error, address))?;
		let port = listener
			.local_addr()
			.map_err(|_| PiError::new("OAuth callback server did not bind to TCP"))?
			.port();
		let socket = Rc::new(RefCell::new(Some(listener)));
		let connections = Rc::new(RefCell::new(HashMap::new()));
		let (accepting, registration) = AbortHandle::new_pair();
		pi_host::spawn_local({
			let accept = accept_connections(socket.clone(), connections.clone(), handler, on_error);
			async move {
				let _ = Abortable::new(accept, registration).await;
			}
		});
		Ok(LoopbackServer {
			port,
			socket,
			accepting,
			connections,
		})
	}

	fn listen_error(error: &std::io::Error, address: SocketAddr) -> PiError {
		let (code, description) = uv_error(error);
		PiError::new(format!(
			"listen {code}: {description} {}:{}",
			address.ip(),
			address.port()
		))
		.with_data(json!({
			"errno": error.raw_os_error().map(|errno| -errno),
			"code": code,
			"syscall": "listen",
			"address": address.ip().to_string(),
			"port": address.port(),
		}))
	}

	/// libuv's code and description for the errors a listening socket reports.
	fn uv_error(error: &std::io::Error) -> (&'static str, &'static str) {
		match error.kind() {
			ErrorKind::AddrInUse => ("EADDRINUSE", "address already in use"),
			ErrorKind::AddrNotAvailable => ("EADDRNOTAVAIL", "address not available"),
			ErrorKind::PermissionDenied => ("EACCES", "permission denied"),
			ErrorKind::InvalidInput => ("EINVAL", "invalid argument"),
			ErrorKind::ConnectionAborted => ("ECONNABORTED", "software caused connection abort"),
			ErrorKind::ConnectionReset => ("ECONNRESET", "connection reset by peer"),
			_ => ("UNKNOWN", "unknown error"),
		}
	}

	const EMFILE: i32 = 24;
	const ENFILE: i32 = 23;
	const DESCRIPTOR_RETRY_MS: f64 = 100.0;

	async fn accept_connections(
		socket: Rc<RefCell<Option<TcpListener>>>,
		connections: Rc<RefCell<HashMap<u64, AbortHandle>>>,
		handler: LoopbackHandler,
		on_error: LoopbackErrorHandler,
	) {
		let next_id = Cell::new(0u64);
		loop {
			let accepted = poll_fn(|cx| match socket.borrow().as_ref() {
				Some(listener) => listener.poll_accept(cx).map(Some),
				None => Poll::Ready(None),
			})
			.await;
			match accepted {
				None => return,
				Some(Ok((stream, _))) => {
					let id = next_id.get();
					next_id.set(id + 1);
					let (connection, registration) = AbortHandle::new_pair();
					connections.borrow_mut().insert(id, connection);
					let handler = handler.clone();
					let connections = connections.clone();
					pi_host::spawn_local(async move {
						let _ = Abortable::new(serve(&handler, stream), registration).await;
						connections.borrow_mut().remove(&id);
					});
				}
				// libuv retries an aborted connection and waits out descriptor exhaustion without reporting either.
				Some(Err(error)) if error.kind() == ErrorKind::ConnectionAborted => {}
				Some(Err(error)) if matches!(error.raw_os_error(), Some(EMFILE | ENFILE)) => {
					let _ = pi_host::sleep(DESCRIPTOR_RETRY_MS, None).await;
				}
				// Node emits `error` on the server and keeps listening.
				Some(Err(error)) => {
					let (code, _) = uv_error(&error);
					on_error(PiError::new(format!("accept {code}")).with_data(json!({
						"errno": error.raw_os_error().map(|errno| -errno),
						"code": code,
						"syscall": "accept",
					})));
				}
			}
		}
	}

	/// Node's `maxHeaderSize`.
	const MAX_REQUEST_HEAD_BYTES: usize = 16 * 1024;
	/// Node's default `headersTimeout`.
	const HEADERS_TIMEOUT_MS: f64 = 60_000.0;

	enum RequestHead {
		Complete(Vec<u8>),
		/// A line ends in a bare LF, which Node's parser rejects.
		Malformed,
		TooLarge,
		Closed,
	}

	async fn read_head(stream: &mut TcpStream) -> RequestHead {
		let mut head = Vec::new();
		let mut chunk = [0u8; 4096];
		loop {
			match stream.read(&mut chunk).await {
				Ok(0) | Err(_) => return RequestHead::Closed,
				Ok(read) => head.extend_from_slice(&chunk[..read]),
			}
			let end = head.windows(4).position(|window| window == b"\r\n\r\n");
			let scanned = &head[..end.map_or(head.len(), |end| end + 4)];
			if scanned
				.iter()
				.enumerate()
				.any(|(index, byte)| *byte == b'\n' && (index == 0 || scanned[index - 1] != b'\r'))
			{
				return RequestHead::Malformed;
			}
			if scanned.len() > MAX_REQUEST_HEAD_BYTES {
				return RequestHead::TooLarge;
			}
			if let Some(end) = end {
				head.truncate(end + 4);
				return RequestHead::Complete(head);
			}
		}
	}

	/// The methods Node's HTTP parser accepts.
	const METHODS: &[&str] = &[
		"DELETE",
		"GET",
		"HEAD",
		"POST",
		"PUT",
		"CONNECT",
		"OPTIONS",
		"TRACE",
		"COPY",
		"LOCK",
		"MKCOL",
		"MOVE",
		"PROPFIND",
		"PROPPATCH",
		"SEARCH",
		"UNLOCK",
		"BIND",
		"REBIND",
		"UNBIND",
		"ACL",
		"REPORT",
		"MKACTIVITY",
		"CHECKOUT",
		"MERGE",
		"M-SEARCH",
		"NOTIFY",
		"SUBSCRIBE",
		"UNSUBSCRIBE",
		"PATCH",
		"PURGE",
		"MKCALENDAR",
		"LINK",
		"UNLINK",
		"SOURCE",
		"QUERY",
	];

	/// The request line, when Node's parser accepts it.
	fn request_line(head: &[u8]) -> Option<LoopbackRequest> {
		let line = &head[..head.windows(2).position(|window| window == b"\r\n")?];
		let mut parts = line.split(|byte| *byte == b' ');
		let method = std::str::from_utf8(parts.next()?).ok()?;
		let target = parts.next()?;
		let version = parts.next()?;
		if parts.next().is_some()
			|| !METHODS.contains(&method)
			|| target.is_empty()
			|| !target.iter().all(|byte| (0x21..=0x7e).contains(byte))
			|| !version.starts_with(b"HTTP/1.")
		{
			return None;
		}
		Some(LoopbackRequest {
			method: method.to_string(),
			target: String::from_utf8(target.to_vec()).ok()?,
		})
	}

	/// The bare response Node's parser sends for a request it rejects.
	async fn reject(stream: &mut TcpStream, status: &str) {
		let _ = stream
			.write_all(format!("HTTP/1.1 {status}\r\nConnection: close\r\n\r\n").as_bytes())
			.await;
		let _ = stream.shutdown().await;
	}

	async fn respond(stream: &mut TcpStream, method: &str, response: LoopbackResponse) {
		let reason = match response.status {
			200 => "OK",
			400 => "Bad Request",
			404 => "Not Found",
			409 => "Conflict",
			500 => "Internal Server Error",
			_ => "Bad Gateway",
		};
		let mut bytes = format!("HTTP/1.1 {} {reason}\r\n", response.status);
		for (name, value) in &response.headers {
			bytes.push_str(&format!("{name}: {value}\r\n"));
		}
		bytes.push_str(&format!(
			"content-length: {}\r\nconnection: close\r\n\r\n",
			response.body.len()
		));
		if method != "HEAD" {
			bytes.push_str(&response.body);
		}
		let _ = stream.write_all(bytes.as_bytes()).await;
		let _ = stream.shutdown().await;
	}

	async fn serve(handler: &LoopbackHandler, mut stream: TcpStream) {
		let head = {
			let read = read_head(&mut stream);
			let deadline = pi_host::sleep(HEADERS_TIMEOUT_MS, None);
			futures::pin_mut!(read, deadline);
			match select(read, deadline).await {
				Either::Left((head, _)) => head,
				Either::Right(_) => return,
			}
		};
		let head = match head {
			RequestHead::Complete(head) => head,
			RequestHead::Malformed => return reject(&mut stream, "400 Bad Request").await,
			RequestHead::TooLarge => return reject(&mut stream, "431 Request Header Fields Too Large").await,
			RequestHead::Closed => return,
		};
		let Some(request) = request_line(&head) else {
			return reject(&mut stream, "400 Bad Request").await;
		};
		let method = request.method.clone();
		let response = handler(request).await;
		respond(&mut stream, &method, response).await;
	}
}

/// The prompt of `waitForCallbackOrManualInput`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ManualInputPrompt {
	pub message: String,
	pub placeholder: String,
}

/// The result of `waitForCallbackOrManualInput`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CallbackOrManualInput<T> {
	Callback { value: T },
	Manual { input: String },
}

/// Wait for the browser callback, or for the user to paste the code or redirect URL when the browser
/// cannot reach the loopback server (for example over SSH). Without a callback server only the manual
/// prompt is used.
pub async fn wait_for_callback_or_manual_input<T: Clone + 'static>(
	interaction: &ProviderAuthInteraction,
	callback: Option<&OAuthCallbackServer<T>>,
	prompt: ManualInputPrompt,
) -> PiResult<CallbackOrManualInput<T>> {
	let manual_abort = AbortController::new();
	let _finally = AbortOnDrop(manual_abort.clone());
	let manual_error: Rc<RefCell<Option<PiError>>> = Rc::new(RefCell::new(None));
	let prompting = interaction.prompt(AuthPrompt {
		signal: Some(manual_abort.signal()),
		kind: AuthPromptKind::ManualCode {
			message: prompt.message,
			placeholder: Some(prompt.placeholder),
		},
	});
	let (manual_sender, manual) = oneshot::channel::<Option<String>>();
	{
		let callback = callback.cloned();
		let manual_error = manual_error.clone();
		pi_host::spawn_local(async move {
			let input = match prompting.await {
				Ok(input) => Some(input),
				Err(error) => {
					*manual_error.borrow_mut() = Some(error);
					None
				}
			};
			if let Some(callback) = &callback {
				callback.cancel();
			}
			let _ = manual_sender.send(input);
		});
	}
	async {
		let value = match callback {
			Some(callback) => callback.wait().await?,
			None => None,
		};
		if let Some(error) = manual_error.borrow_mut().take() {
			return Err(error);
		}
		if let Some(value) = value {
			return Ok(CallbackOrManualInput::Callback { value });
		}
		let input = manual.await.ok().flatten();
		if let Some(error) = manual_error.borrow_mut().take() {
			return Err(error);
		}
		Ok(CallbackOrManualInput::Manual {
			input: input.unwrap_or_default(),
		})
	}
	.await
}

/// Runs the TS `finally { manualAbort.abort() }` on every exit, a dropped future included.
struct AbortOnDrop(AbortController);

impl Drop for AbortOnDrop {
	fn drop(&mut self) {
		self.0.abort();
	}
}
