//! Port of `packages/mcp/src/oauth/callback.ts`.
//! PORT-STATUS: reviewed
//!
//! Behind the `native` feature: the loopback HTTP server is a tokio `TcpListener` that reads one request
//! head per connection and answers with `connection: close`, instead of `node:http`. `OAuthCallbackPage` is
//! an enum: `Ok` for `{ ok: true }` and `Failed` for `{ ok: false, message, details }`. A request head must
//! arrive within 60 seconds, like Node's `headersTimeout`. `close` stops accepting and drops open connections
//! at once instead of waiting for them. A request target that is not a valid URL gets a 404 page, where the
//! TS handler throws. A bind failure is an `Error` with Node's `listen <code>: <description> <host>:<port>`
//! message and `data.code`.

#![cfg(feature = "native")]

use std::cell::RefCell;
use std::rc::Rc;

use futures::channel::oneshot;
use futures::future::{AbortHandle, Abortable, Either, select};
use indexmap::IndexMap;
use pi_host::{LocalBoxFuture, PiError, PiResult};
use serde_json::json;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use url::Url;

const MAX_REQUEST_HEAD_BYTES: usize = 64 * 1024;
/// Node's default `headersTimeout`.
const HEADERS_TIMEOUT_MS: f64 = 60_000.0;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OAuthCallback {
	pub code: String,
	pub state: String,
	pub iss: Option<String>,
}

/// Outcome shown on the browser page after the redirect.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OAuthCallbackPage {
	Ok,
	Failed { message: String, details: Option<String> },
}

/// `renderPage(page)`.
pub type RenderPageFunction = Rc<dyn Fn(&OAuthCallbackPage) -> String>;

#[derive(Clone, Default)]
pub struct OAuthCallbackServerOptions {
	/// Address to listen on. Default: `127.0.0.1`.
	pub host: Option<String>,
	/// Host name in `redirect_url`, for example `localhost` for a client registered with it while
	/// listening on `127.0.0.1`. Default: `host`.
	pub redirect_host: Option<String>,
	pub port: Option<u16>,
	pub path: Option<String>,
	/// More paths that receive the callback, for example a server-specific path of a redirect URI.
	pub extra_paths: Option<Vec<String>>,
	pub timeout_ms: Option<f64>,
	/// Render the browser page as HTML. Default: a plain-text message.
	pub render_page: Option<RenderPageFunction>,
}

fn plain_text(page: &OAuthCallbackPage) -> String {
	match page {
		OAuthCallbackPage::Ok => "Authorization complete. You may close this window.".into(),
		OAuthCallbackPage::Failed { message, details } => match details {
			Some(details) if !details.is_empty() => format!("{message}\n\n{details}"),
			_ => message.clone(),
		},
	}
}

struct Pending {
	sender: oneshot::Sender<PiResult<OAuthCallback>>,
	timer: AbortHandle,
	path: Option<String>,
}

struct Inner {
	redirect_url: String,
	paths: Vec<String>,
	timeout_ms: f64,
	render_page: Option<RenderPageFunction>,
	pending: RefCell<IndexMap<String, Pending>>,
	listener: AbortHandle,
	connections: RefCell<Vec<AbortHandle>>,
}

pub struct OAuthCallbackServer {
	inner: Rc<Inner>,
}

impl OAuthCallbackServer {
	/// `OAuthCallbackServer.listen(options)`.
	pub async fn listen(options: OAuthCallbackServerOptions) -> PiResult<Self> {
		let host = options.host.unwrap_or_else(|| "127.0.0.1".into());
		let redirect_host = options.redirect_host.unwrap_or_else(|| host.clone());
		let path = options.path.unwrap_or_else(|| "/callback".into());
		let port = options.port.unwrap_or(0);
		let listener = TcpListener::bind((host.as_str(), port))
			.await
			.map_err(|error| listen_error(&error, &host, port))?;
		let port = listener
			.local_addr()
			.map_err(|_| PiError::new("OAuth callback server did not bind to TCP"))?
			.port();
		let redirect_host = if redirect_host.contains(':') {
			format!("[{redirect_host}]")
		} else {
			redirect_host
		};
		let (listener_handle, registration) = AbortHandle::new_pair();
		let mut paths = vec![path.clone()];
		paths.extend(options.extra_paths.unwrap_or_default());
		let inner = Rc::new(Inner {
			redirect_url: format!("http://{redirect_host}:{port}{path}"),
			paths,
			timeout_ms: options.timeout_ms.unwrap_or(5.0 * 60_000.0),
			render_page: options.render_page,
			pending: RefCell::new(IndexMap::new()),
			listener: listener_handle,
			connections: RefCell::new(Vec::new()),
		});
		let accepting = Rc::downgrade(&inner);
		pi_host::spawn_local(async move {
			let accept = async move {
				while let Ok((stream, _)) = listener.accept().await {
					let Some(inner) = accepting.upgrade() else {
						return;
					};
					let (connection, registration) = AbortHandle::new_pair();
					inner
						.connections
						.borrow_mut()
						.retain(|connection| !connection.is_aborted());
					inner.connections.borrow_mut().push(connection.clone());
					pi_host::spawn_local(async move {
						let _ = Abortable::new(serve(&inner, stream), registration).await;
						connection.abort();
					});
				}
			};
			let _ = Abortable::new(accept, registration).await;
		});
		Ok(Self { inner })
	}

	pub fn redirect_url(&self) -> &str {
		&self.inner.redirect_url
	}

	/// Wait for the authorization response with `state`. With `path`, a response on another path fails, so
	/// a server-specific redirect URI can tell authorization servers apart (RFC 9700 section 4.4.2.2).
	pub fn wait_for_callback(
		&self,
		state: &str,
		path: Option<&str>,
	) -> PiResult<LocalBoxFuture<'static, PiResult<OAuthCallback>>> {
		let inner = &self.inner;
		if inner.pending.borrow().contains_key(state) {
			return Err(PiError::new("OAuth state is already pending"));
		}
		let (sender, receiver) = oneshot::channel();
		let (timer, registration) = AbortHandle::new_pair();
		let expired = Rc::downgrade(inner);
		let timeout_ms = inner.timeout_ms;
		let key = state.to_string();
		pi_host::spawn_local(async move {
			let wait = Abortable::new(pi_host::sleep(timeout_ms, None), registration);
			if wait.await.is_err() {
				return;
			}
			let Some(inner) = expired.upgrade() else {
				return;
			};
			let pending = inner.pending.borrow_mut().shift_remove(&key);
			if let Some(pending) = pending {
				let _ = pending.sender.send(Err(PiError::new("OAuth callback timed out")));
			}
		});
		inner.pending.borrow_mut().insert(
			state.to_string(),
			Pending {
				sender,
				timer,
				path: path.map(str::to_string),
			},
		);
		Ok(Box::pin(async move {
			receiver
				.await
				.unwrap_or_else(|_| Err(PiError::new("OAuth callback server closed")))
		}))
	}

	/// `close()`: rejects every pending callback and stops listening.
	pub async fn close(&self) -> PiResult<()> {
		let pending = std::mem::take(&mut *self.inner.pending.borrow_mut());
		for (_, pending) in pending {
			pending.timer.abort();
			let _ = pending.sender.send(Err(PiError::new("OAuth callback server closed")));
		}
		self.inner.listener.abort();
		for connection in std::mem::take(&mut *self.inner.connections.borrow_mut()) {
			connection.abort();
		}
		Ok(())
	}
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

async fn serve(inner: &Inner, mut stream: TcpStream) {
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
	let target = head
		.lines()
		.next()
		.and_then(|line| line.split_whitespace().nth(1))
		.unwrap_or("/");
	let (status, page) = handle(inner, target);
	let (content_type, body) = match &inner.render_page {
		Some(render_page) => (
			"text/html; charset=utf-8\r\ncache-control: no-store",
			render_page(&page),
		),
		None => ("text/plain; charset=utf-8", plain_text(&page)),
	};
	let reason = match status {
		200 => "OK",
		400 => "Bad Request",
		_ => "Not Found",
	};
	let response = format!(
		"HTTP/1.1 {status} {reason}\r\ncontent-type: {content_type}\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
		body.len()
	);
	let _ = stream.write_all(response.as_bytes()).await;
	let _ = stream.shutdown().await;
}

fn failed(message: &str) -> OAuthCallbackPage {
	OAuthCallbackPage::Failed {
		message: message.into(),
		details: None,
	}
}

fn handle(inner: &Inner, raw_url: &str) -> (u16, OAuthCallbackPage) {
	let Some(url) = Url::parse(&inner.redirect_url)
		.ok()
		.and_then(|base| base.join(raw_url).ok())
	else {
		return (404, failed("Not found"));
	};
	if !inner.paths.iter().any(|path| path == url.path()) {
		return (404, failed("Not found"));
	}
	let param = |name: &str| {
		url.query_pairs()
			.find(|(key, _)| key == name)
			.map(|(_, value)| value.into_owned())
	};
	let state = param("state").filter(|state| !state.is_empty());
	let pending = state
		.as_ref()
		.and_then(|state| inner.pending.borrow_mut().shift_remove(state));
	let (Some(state), Some(pending)) = (state, pending) else {
		return (400, failed("Invalid or expired OAuth state"));
	};
	pending.timer.abort();
	if let Some(path) = &pending.path
		&& url.path() != path
	{
		let _ = pending.sender.send(Err(PiError::new(
			"The authorization response arrived on another redirect URI",
		)));
		return (400, failed("Unexpected redirect URI"));
	}
	if let Some(error) = param("error").filter(|error| !error.is_empty()) {
		let description = param("error_description").unwrap_or(error);
		let _ = pending.sender.send(Err(PiError::new(description.clone())));
		return (
			200,
			OAuthCallbackPage::Failed {
				message: "Authorization failed. You may close this window.".into(),
				details: Some(description),
			},
		);
	}
	let Some(code) = param("code").filter(|code| !code.is_empty()) else {
		let _ = pending.sender.send(Err(PiError::new(
			"OAuth callback did not include an authorization code",
		)));
		return (400, failed("Missing authorization code"));
	};
	let iss = param("iss").filter(|iss| !iss.is_empty());
	let _ = pending.sender.send(Ok(OAuthCallback { code, state, iss }));
	(200, OAuthCallbackPage::Ok)
}
