//! Port of `packages/ai/src/auth/oauth/callback-server.ts`.
//! PORT-STATUS: reviewed
//!
//! Loopback OAuth redirect handler shared by the browser sign-in flows.
//!
//! The listening socket is the host's HTTP listener (`pi_host::server::listen_http`): a tokio `TcpListener` on the
//! native host, `node:http` on the JS hosts, and a rejection on hosts without one; flows that catch the rejection
//! continue with manual input. The server binds the first address the host name resolves to, fails with Node's
//! listen error instead of picking another port, closes its socket inside `close()`, and keeps the wait alive while
//! its callbacks can still settle it, so dropping the server handle neither rejects `wait()` nor stops the server.
//! An error of the server after it listens settles the wait, as Node's `error` event does. A request
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
use pi_host::server::{HttpServerHandler, HttpServerRequest, HttpServerResponse, listen_http};
use pi_host::{AbortController, AbortSignal, LocalBoxFuture, PiError, PiResult};
use url::Url;

use crate::auth::types::{AuthPrompt, AuthPromptKind, ProviderAuthInteraction};
use crate::utils::oauth_page::{oauth_error_html, oauth_success_html};

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

	let port = listen(&options, state.clone()).await?;

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

/// Starts the host's HTTP listener with the callback handler; returns the bound port.
async fn listen<T: 'static>(options: &OAuthCallbackServerOptions<T>, state: Rc<CallbackState<T>>) -> PiResult<u16> {
	let provider_name = options.provider_name.clone();
	let path = options.path.clone();
	let expected_state = options.state.clone();
	let complete = options.complete.clone();
	let callback = state.clone();
	let handler: HttpServerHandler = Rc::new(move |request: HttpServerRequest| {
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
					let html = oauth_error_html(&format!("{provider_name} sign-in failed."), Some(&failure.message));
					callback.finish(Err(failure));
					page(502, html)
				}
			}
		}) as LocalBoxFuture<'static, HttpServerResponse>
	});
	let on_error = {
		let state = state.clone();
		Rc::new(move |error: PiError| state.finish(Err(error)))
	};
	let server = listen_http(&options.host, options.port, handler, on_error).await?;
	let port = server.port();
	*state.close_listener.borrow_mut() = Some(Box::new(move || server.close()));
	Ok(port)
}

fn page(status: u16, html: String) -> HttpServerResponse {
	HttpServerResponse {
		status,
		headers: vec![
			("content-type".into(), "text/html; charset=utf-8".into()),
			("cache-control".into(), "no-store".into()),
		],
		body: html,
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
