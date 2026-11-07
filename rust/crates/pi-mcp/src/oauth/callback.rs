//! Port of `packages/mcp/src/oauth/callback.ts`.
//! PORT-STATUS: reviewed
//!
//! The loopback HTTP server is the host's `listen_http` (`pi_host::server`), the counterpart of `node:http`: the
//! native host serves one request per connection with `connection: close`, the JS host uses `node:http`, and hosts
//! without a listener fail `listen`. A server error after it listens is ignored, as the TS server has no `error`
//! listener once it listens.
//! `OAuthCallbackPage` is an enum: `Ok` for `{ ok: true }` and `Failed` for `{ ok: false, message, details }`. A
//! request target that is not a valid URL gets a 404 page, where the TS handler throws. A bind failure is the
//! host's `Error` with Node's `listen <code>: <description> <host>:<port>` message and `data.code`.

use std::cell::RefCell;
use std::rc::Rc;

use futures::channel::oneshot;
use futures::future::{AbortHandle, Abortable};
use indexmap::IndexMap;
use pi_host::server::{HttpServer, HttpServerRequest, HttpServerResponse};
use pi_host::{LocalBoxFuture, PiError, PiResult};
use url::Url;

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
}

pub struct OAuthCallbackServer {
	inner: Rc<Inner>,
	server: Rc<dyn HttpServer>,
}

impl OAuthCallbackServer {
	/// `OAuthCallbackServer.listen(options)`.
	pub async fn listen(options: OAuthCallbackServerOptions) -> PiResult<Self> {
		let host = options.host.unwrap_or_else(|| "127.0.0.1".into());
		let redirect_host = options.redirect_host.unwrap_or_else(|| host.clone());
		let path = options.path.unwrap_or_else(|| "/callback".into());
		// The handler runs only after `listen` resolved and `inner` was set, like the TS `instance?.handle`.
		let instance: Rc<RefCell<Option<Rc<Inner>>>> = Rc::default();
		let handler_instance = instance.clone();
		let server = pi_host::server::listen_http(
			&host,
			options.port.unwrap_or(0),
			Rc::new(move |request: HttpServerRequest| {
				let inner = handler_instance.borrow().clone();
				let response = match inner {
					Some(inner) => reply(&inner, &request.target),
					None => HttpServerResponse {
						status: 404,
						headers: Vec::new(),
						body: String::new(),
					},
				};
				Box::pin(async move { response })
			}),
			Rc::new(|_| {}),
		)
		.await?;
		let redirect_host = if redirect_host.contains(':') {
			format!("[{redirect_host}]")
		} else {
			redirect_host
		};
		let mut paths = vec![path.clone()];
		paths.extend(options.extra_paths.unwrap_or_default());
		let inner = Rc::new(Inner {
			redirect_url: format!("http://{redirect_host}:{}{path}", server.port()),
			paths,
			timeout_ms: options.timeout_ms.unwrap_or(5.0 * 60_000.0),
			render_page: options.render_page,
			pending: RefCell::new(IndexMap::new()),
		});
		*instance.borrow_mut() = Some(inner.clone());
		Ok(Self { inner, server })
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
		self.server.close();
		Ok(())
	}
}

/// The response to a request for `raw_url`: the page, rendered by `renderPage` or as plain text.
fn reply(inner: &Inner, raw_url: &str) -> HttpServerResponse {
	let (status, page) = handle(inner, raw_url);
	let mut headers = Vec::new();
	let body = match &inner.render_page {
		Some(render_page) => {
			headers.push(("content-type".into(), "text/html; charset=utf-8".into()));
			headers.push(("cache-control".into(), "no-store".into()));
			render_page(&page)
		}
		None => {
			headers.push(("content-type".into(), "text/plain; charset=utf-8".into()));
			plain_text(&page)
		}
	};
	HttpServerResponse { status, headers, body }
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
