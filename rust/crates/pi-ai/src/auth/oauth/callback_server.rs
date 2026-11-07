//! Port of `packages/ai/src/auth/oauth/callback-server.ts`.
//! PORT-STATUS: ported
//!
//! Loopback OAuth redirect handler shared by the browser sign-in flows.
//!
//! The listening socket needs the `native` feature: it is a tokio `TcpListener` that reads one request head per
//! connection (within 60 seconds, Node's `headersTimeout`) and answers with `connection: close`, instead of
//! `node:http`. Without `native`, `startOAuthCallbackServer` rejects with
//! `OAuth callback server requires the native feature`, as a platform that cannot listen would; flows that catch the
//! rejection continue with manual input. A request target that is not a valid URL gets the 404 page, where the TS
//! handler throws and never answers. `close` stops accepting; open connections finish their response, like Node's
//! `server.close()`. A bind failure is an `Error` with Node's `listen <code>: <description> <host>:<port>` message
//! and `data.code`. The wait promise is a `Shared` future, so `T` is `Clone`. TS returns the anonymous
//! `{ type: "callback", value } | { type: "manual", input }` union of `waitForCallbackOrManualInput` as
//! `CallbackOrManualInput` and takes its prompt as `ManualInputPrompt`.

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
	listener: RefCell<Option<AbortHandle>>,
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
		if let Some(listener) = self.state.listener.borrow_mut().take() {
			listener.abort();
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
		listener: RefCell::new(None),
	});

	let port = native::listen(&options, state.clone()).await?;

	if let Some(signal) = &options.signal {
		let aborted = Rc::downgrade(&state);
		let listener = signal.add_listener(move |_| {
			if let Some(state) = aborted.upgrade() {
				state.finish(Err(PiError::new("Login cancelled")));
			}
		});
		*state.abort_listener.borrow_mut() = Some((signal.clone(), listener));
	}
	if let Some(timeout_ms) = options.timeout_ms {
		let (timer, registration) = AbortHandle::new_pair();
		*state.timer.borrow_mut() = Some(timer);
		let expired = Rc::downgrade(&state);
		let message = format!("{} sign-in timed out", options.provider_name);
		pi_host::spawn_local(async move {
			if futures::future::Abortable::new(pi_host::sleep(timeout_ms, None), registration)
				.await
				.is_err()
			{
				return;
			}
			if let Some(state) = expired.upgrade() {
				state.finish(Err(PiError::new(message)));
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
	use std::rc::{Rc, Weak};

	use futures::future::{AbortHandle, Abortable, Either, select};
	use pi_host::{PiError, PiResult};
	use serde_json::json;
	use tokio::io::{AsyncReadExt, AsyncWriteExt};
	use tokio::net::{TcpListener, TcpStream};
	use url::Url;

	use super::{CallbackState, OAuthCallbackCompleteFunction, OAuthCallbackServerOptions};
	use crate::utils::oauth_page::{oauth_error_html, oauth_success_html};

	const MAX_REQUEST_HEAD_BYTES: usize = 64 * 1024;
	/// Node's default `headersTimeout`.
	const HEADERS_TIMEOUT_MS: f64 = 60_000.0;

	struct Route<T> {
		provider_name: String,
		path: String,
		state: Option<String>,
		complete: OAuthCallbackCompleteFunction<T>,
		callback: Weak<CallbackState<T>>,
	}

	/// Binds the socket and starts accepting; returns the bound port.
	pub(super) async fn listen<T: 'static>(
		options: &OAuthCallbackServerOptions<T>,
		state: Rc<CallbackState<T>>,
	) -> PiResult<u16> {
		let listener = TcpListener::bind((options.host.as_str(), options.port))
			.await
			.map_err(|error| listen_error(&error, &options.host, options.port))?;
		let port = listener
			.local_addr()
			.map_err(|_| PiError::new("OAuth callback server did not bind to TCP"))?
			.port();
		let route = Rc::new(Route {
			provider_name: options.provider_name.clone(),
			path: options.path.clone(),
			state: options.state.clone(),
			complete: options.complete.clone(),
			callback: Rc::downgrade(&state),
		});
		let (handle, registration) = AbortHandle::new_pair();
		*state.listener.borrow_mut() = Some(handle);
		pi_host::spawn_local(async move {
			let accept = async move {
				loop {
					match listener.accept().await {
						Ok((stream, _)) => {
							let route = route.clone();
							pi_host::spawn_local(async move { serve(&route, stream).await });
						}
						Err(error) => {
							if let Some(callback) = route.callback.upgrade() {
								callback.finish(Err(PiError::new(error.to_string())));
							}
							return;
						}
					}
				}
			};
			let _ = Abortable::new(accept, registration).await;
		});
		Ok(port)
	}

	fn listen_error(error: &std::io::Error, host: &str, port: u16) -> PiError {
		let (code, description) = match error.kind() {
			std::io::ErrorKind::AddrInUse => ("EADDRINUSE", "address already in use"),
			std::io::ErrorKind::AddrNotAvailable => ("EADDRNOTAVAIL", "address not available"),
			std::io::ErrorKind::PermissionDenied => ("EACCES", "permission denied"),
			_ => ("EIO", "i/o error"),
		};
		PiError::new(format!("listen {code}: {description} {host}:{port}")).with_data(json!({ "code": code }))
	}

	async fn read_head(stream: &mut TcpStream) -> Option<Vec<u8>> {
		let mut head = Vec::new();
		let mut chunk = [0u8; 4096];
		while !head.windows(4).any(|window| window == b"\r\n\r\n") {
			match stream.read(&mut chunk).await {
				Ok(0) | Err(_) => return None,
				Ok(read) => head.extend_from_slice(&chunk[..read]),
			}
			if head.len() > MAX_REQUEST_HEAD_BYTES {
				return None;
			}
		}
		Some(head)
	}

	async fn send_page(stream: &mut TcpStream, status: u16, html: String) {
		let reason = match status {
			200 => "OK",
			400 => "Bad Request",
			404 => "Not Found",
			409 => "Conflict",
			_ => "Bad Gateway",
		};
		let response = format!(
			"HTTP/1.1 {status} {reason}\r\ncontent-type: text/html; charset=utf-8\r\ncache-control: no-store\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{html}",
			html.len()
		);
		let _ = stream.write_all(response.as_bytes()).await;
		let _ = stream.shutdown().await;
	}

	async fn serve<T>(route: &Route<T>, mut stream: TcpStream) {
		let head = {
			let read = read_head(&mut stream);
			let deadline = pi_host::sleep(HEADERS_TIMEOUT_MS, None);
			futures::pin_mut!(read, deadline);
			match select(read, deadline).await {
				Either::Left((Some(head), _)) => head,
				_ => return,
			}
		};
		let head = String::from_utf8_lossy(&head);
		let mut request_line = head.lines().next().unwrap_or("").split_whitespace();
		let method = request_line.next().unwrap_or("");
		let target = request_line.next().unwrap_or("/");
		let url = Url::parse("http://localhost").and_then(|base| base.join(target));
		let url = match url {
			Ok(url) if method == "GET" && url.path() == route.path => url,
			_ => return send_page(&mut stream, 404, oauth_error_html("Callback route not found.", None)).await,
		};
		let param = |name: &str| {
			url.query_pairs()
				.find(|(key, _)| key == name)
				.map(|(_, value)| value.into_owned())
		};
		if let Some(expected) = &route.state
			&& param("state").as_ref() != Some(expected)
		{
			return send_page(&mut stream, 400, oauth_error_html("State mismatch.", None)).await;
		}
		let Some(callback) = route.callback.upgrade() else {
			return send_page(
				&mut stream,
				409,
				oauth_error_html("This sign-in has already been handled.", None),
			)
			.await;
		};
		if callback.claimed.get() || callback.settled.get() {
			return send_page(
				&mut stream,
				409,
				oauth_error_html("This sign-in has already been handled.", None),
			)
			.await;
		}
		let provider_name = &route.provider_name;
		if let Some(error) = param("error").filter(|error| !error.is_empty()) {
			let description = param("error_description").unwrap_or(error);
			send_page(
				&mut stream,
				400,
				oauth_error_html(&format!("{provider_name} authorization failed."), Some(&description)),
			)
			.await;
			callback.finish(Err(PiError::new(format!(
				"{provider_name} authorization failed: {description}"
			))));
			return;
		}
		let Some(code) = param("code").filter(|code| !code.is_empty()) else {
			return send_page(&mut stream, 400, oauth_error_html("Missing authorization code.", None)).await;
		};
		callback.claimed.set(true);
		match (route.complete)(code).await {
			Ok(value) => {
				send_page(
					&mut stream,
					200,
					oauth_success_html(&format!("Signed in to {provider_name}. You may now close this page.")),
				)
				.await;
				callback.finish(Ok(Some(value)));
			}
			Err(failure) => {
				send_page(
					&mut stream,
					502,
					oauth_error_html(&format!("{provider_name} sign-in failed."), Some(&failure.message)),
				)
				.await;
				callback.finish(Err(failure));
			}
		}
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
	let result = async {
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
	.await;
	manual_abort.abort();
	result
}
