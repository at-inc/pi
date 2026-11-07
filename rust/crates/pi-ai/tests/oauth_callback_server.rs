//! Port of `packages/ai/test/oauth-callback-server.test.ts`.
//!
//! Needs the `native` feature: the tests run on the tokio host and reach the real loopback socket through
//! `pi_host::fetch`. Tests whose server renders a page wait for `utils/oauth_page.rs` (w2-ai-core).

#![cfg(feature = "native")]

use std::cell::RefCell;
use std::rc::Rc;

use futures::channel::oneshot;
use pi_ai::auth::oauth::callback_server::{
	CallbackOrManualInput, ManualInputPrompt, OAuthCallbackServer, OAuthCallbackServerOptions,
	start_oauth_callback_server, wait_for_callback_or_manual_input,
};
use pi_ai::auth::types::{AuthPrompt, ProviderAuthInteraction};
use pi_host::{AbortController, AbortSignal, HttpRequest, LocalBoxFuture, PiError, PiResult};
use url::Url;

fn callback_url(redirect_uri: &str, params: &[(&str, &str)]) -> String {
	let mut url = Url::parse(redirect_uri).unwrap();
	for (key, value) in params {
		url.query_pairs_mut().append_pair(key, value);
	}
	url.to_string()
}

struct Page {
	status: u16,
	content_type: Option<String>,
	body: String,
}

async fn fetch(method: &str, url: &str) -> Page {
	let response = pi_host::fetch(HttpRequest::new(method, url), None).await.unwrap();
	let status = response.status;
	let content_type = response.header("content-type").map(str::to_string);
	Page {
		status,
		content_type,
		body: response.text().await.unwrap(),
	}
}

fn interaction(
	prompt: impl Fn(AuthPrompt) -> LocalBoxFuture<'static, PiResult<String>> + 'static,
	signal: Option<AbortSignal>,
) -> ProviderAuthInteraction {
	ProviderAuthInteraction {
		signal: signal.unwrap_or_else(|| AbortController::new().signal()),
		notify: Rc::new(|_| {}),
		prompt: Rc::new(prompt),
	}
}

/// A manual prompt that stays open until its signal aborts.
fn pending_prompt(
	on_prompt: impl Fn(&AuthPrompt) + 'static,
) -> impl Fn(AuthPrompt) -> LocalBoxFuture<'static, PiResult<String>> + 'static {
	move |prompt| {
		on_prompt(&prompt);
		let signal = prompt.signal.clone().expect("manual prompts carry a signal");
		Box::pin(async move {
			signal.aborted_future().await;
			Err(PiError::new("prompt aborted"))
		})
	}
}

fn options() -> OAuthCallbackServerOptions<String> {
	OAuthCallbackServerOptions {
		provider_name: "Example".into(),
		host: "127.0.0.1".into(),
		port: 0,
		path: "/callback".into(),
		redirect_host: None,
		state: Some("expected-state".into()),
		complete: Rc::new(|code| Box::pin(async move { Ok(format!("completed:{code}")) })),
		signal: None,
		timeout_ms: None,
	}
}

fn plain_options() -> OAuthCallbackServerOptions<String> {
	OAuthCallbackServerOptions {
		state: None,
		complete: Rc::new(|code| Box::pin(async move { Ok(code) })),
		..options()
	}
}

async fn start(options: OAuthCallbackServerOptions<String>) -> PiResult<OAuthCallbackServer<String>> {
	start_oauth_callback_server(options).await
}

mod oauth_callback_server {
	use super::*;

	#[test]
	#[ignore = "PORT-STUB: needs utils/oauth_page.rs from w2-ai-core"]
	fn ignores_stray_requests_and_resolves_with_the_completed_code() {
		pi_host::native::run(|| async {
			let server = start(options()).await.unwrap();
			let pattern = regex::Regex::new(r"^http://127\.0\.0\.1:\d+/callback$").unwrap();
			assert!(pattern.is_match(&server.redirect_uri));

			let wrong_path = fetch(
				"GET",
				Url::parse(&server.redirect_uri)
					.unwrap()
					.join("/other")
					.unwrap()
					.as_str(),
			)
			.await;
			assert_eq!(wrong_path.status, 404);
			let wrong_state = fetch(
				"GET",
				&callback_url(&server.redirect_uri, &[("code", "c"), ("state", "other")]),
			)
			.await;
			assert_eq!(wrong_state.status, 400);
			assert_eq!(wrong_state.content_type.as_deref(), Some("text/html; charset=utf-8"));
			assert!(wrong_state.body.contains("State mismatch."));
			let post = fetch(
				"POST",
				&callback_url(&server.redirect_uri, &[("code", "c"), ("state", "expected-state")]),
			)
			.await;
			assert_eq!(post.status, 404);
			let missing_code = fetch(
				"GET",
				&callback_url(&server.redirect_uri, &[("state", "expected-state")]),
			)
			.await;
			assert_eq!(missing_code.status, 400);

			let success = fetch(
				"GET",
				&callback_url(
					&server.redirect_uri,
					&[("code", "the-code"), ("state", "expected-state")],
				),
			)
			.await;
			assert_eq!(success.status, 200);
			assert_eq!(success.content_type.as_deref(), Some("text/html; charset=utf-8"));
			assert!(success.body.contains("Authentication successful"));
			assert!(success.body.contains("Signed in to Example."));
			assert!(success.body.contains("fill=\"#F09082\""));
			assert!(success.body.contains("fill=\"#4D9ABF\""));
			assert!(success.body.contains("fill=\"#F1BE58\""));
			assert_eq!(server.wait().await.unwrap().as_deref(), Some("completed:the-code"));
			server.close();
		});
	}

	#[test]
	#[ignore = "PORT-STUB: needs utils/oauth_page.rs from w2-ai-core"]
	fn uses_the_redirect_host_and_skips_the_state_check_when_none_is_expected() {
		pi_host::native::run(|| async {
			let server = start(OAuthCallbackServerOptions {
				redirect_host: Some("localhost".into()),
				state: None,
				..options()
			})
			.await
			.unwrap();
			let pattern = regex::Regex::new(r"^http://localhost:\d+/callback$").unwrap();
			assert!(pattern.is_match(&server.redirect_uri));
			let response = fetch("GET", &callback_url(&server.redirect_uri, &[("code", "no-state")])).await;
			assert_eq!(response.status, 200);
			assert_eq!(server.wait().await.unwrap().as_deref(), Some("completed:no-state"));
			server.close();
		});
	}

	#[test]
	#[ignore = "PORT-STUB: needs utils/oauth_page.rs from w2-ai-core"]
	fn shows_completion_failures_on_the_page_and_rejects_the_wait() {
		pi_host::native::run(|| async {
			let server = start(OAuthCallbackServerOptions {
				complete: Rc::new(|_| Box::pin(async { Err(PiError::new("token exchange failed")) })),
				..options()
			})
			.await
			.unwrap();
			let failure = fetch(
				"GET",
				&callback_url(&server.redirect_uri, &[("code", "c"), ("state", "expected-state")]),
			)
			.await;
			assert_eq!(failure.status, 502);
			assert!(failure.body.contains("Example sign-in failed."));
			assert!(failure.body.contains("token exchange failed"));
			assert!(
				server
					.wait()
					.await
					.unwrap_err()
					.message
					.contains("token exchange failed")
			);
			server.close();
		});
	}

	#[test]
	#[ignore = "PORT-STUB: needs utils/oauth_page.rs from w2-ai-core"]
	fn rejects_the_wait_when_the_provider_redirects_with_an_error() {
		pi_host::native::run(|| async {
			let server = start(options()).await.unwrap();
			let failure = fetch(
				"GET",
				&callback_url(
					&server.redirect_uri,
					&[
						("error", "access_denied"),
						("error_description", "User denied access"),
						("state", "expected-state"),
					],
				),
			)
			.await;
			assert_eq!(failure.status, 400);
			assert!(failure.body.contains("User denied access"));
			assert!(
				server
					.wait()
					.await
					.unwrap_err()
					.message
					.contains("Example authorization failed: User denied access")
			);
			server.close();
		});
	}

	#[test]
	#[ignore = "PORT-STUB: needs utils/oauth_page.rs from w2-ai-core"]
	fn completes_only_the_first_callback() {
		pi_host::native::run(|| async {
			let finish_exchange: Rc<RefCell<Option<oneshot::Sender<String>>>> = Rc::new(RefCell::new(None));
			let exchange = finish_exchange.clone();
			let server = start(OAuthCallbackServerOptions {
				complete: Rc::new(move |_| {
					let (sender, receiver) = oneshot::channel();
					*exchange.borrow_mut() = Some(sender);
					Box::pin(async move { Ok(receiver.await.unwrap()) })
				}),
				..options()
			})
			.await
			.unwrap();
			let url = callback_url(&server.redirect_uri, &[("code", "c"), ("state", "expected-state")]);
			let (first_sender, first) = oneshot::channel();
			{
				let url = url.clone();
				pi_host::spawn_local(async move {
					let _ = first_sender.send(fetch("GET", &url).await.status);
				});
			}
			while finish_exchange.borrow().is_none() {
				pi_host::sleep(5.0, None).await.unwrap();
			}
			let second = fetch("GET", &url).await;
			assert_eq!(second.status, 409);
			// A claimed callback keeps completing even when the caller switches to manual input.
			server.cancel();
			let sender = finish_exchange.borrow_mut().take().unwrap();
			sender.send("done".into()).unwrap();
			assert_eq!(first.await.unwrap(), 200);
			assert_eq!(server.wait().await.unwrap().as_deref(), Some("done"));
			server.close();
		});
	}

	#[test]
	#[ignore = "PORT-STUB: needs utils/oauth_page.rs from w2-ai-core"]
	fn resolves_with_undefined_after_cancel() {
		pi_host::native::run(|| async {
			let server = start(options()).await.unwrap();
			server.cancel();
			assert_eq!(server.wait().await.unwrap(), None);
			let late = fetch(
				"GET",
				&callback_url(&server.redirect_uri, &[("code", "c"), ("state", "expected-state")]),
			)
			.await;
			assert_eq!(late.status, 409);
			server.close();
		});
	}

	#[test]
	fn rejects_the_wait_on_abort_and_on_timeout() {
		pi_host::native::run(|| async {
			let controller = AbortController::new();
			let aborted = start(OAuthCallbackServerOptions {
				signal: Some(controller.signal()),
				..options()
			})
			.await
			.unwrap();
			controller.abort();
			assert!(aborted.wait().await.unwrap_err().message.contains("Login cancelled"));
			aborted.close();

			let timed_out = start(OAuthCallbackServerOptions {
				timeout_ms: Some(10.0),
				..options()
			})
			.await
			.unwrap();
			assert!(
				timed_out
					.wait()
					.await
					.unwrap_err()
					.message
					.contains("Example sign-in timed out")
			);
			timed_out.close();

			let already_aborted = AbortController::new();
			already_aborted.abort();
			let error = start(OAuthCallbackServerOptions {
				signal: Some(already_aborted.signal()),
				..options()
			})
			.await
			.err()
			.unwrap();
			assert!(error.message.contains("Login cancelled"));
		});
	}

	#[test]
	fn fails_instead_of_picking_another_port_when_the_requested_port_is_taken() {
		pi_host::native::run(|| async {
			let blocker = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
			let port = blocker.local_addr().unwrap().port();
			let error = start(OAuthCallbackServerOptions { port, ..options() })
				.await
				.err()
				.unwrap();
			assert_eq!(error.data.as_ref().unwrap()["code"], "EADDRINUSE");
			drop(blocker);
		});
	}
}

mod wait_for_callback_or_manual_input {
	use super::*;

	#[test]
	#[ignore = "PORT-STUB: needs utils/oauth_page.rs from w2-ai-core"]
	fn returns_the_browser_callback_and_aborts_the_manual_prompt() {
		pi_host::native::run(|| async {
			let manual_signal: Rc<RefCell<Option<AbortSignal>>> = Rc::new(RefCell::new(None));
			let server = start(plain_options()).await.unwrap();
			let recorded = manual_signal.clone();
			let interaction = interaction(
				pending_prompt(move |prompt| *recorded.borrow_mut() = prompt.signal.clone()),
				None,
			);
			let (result_sender, result) = oneshot::channel();
			{
				let server = server.clone();
				pi_host::spawn_local(async move {
					let prompt = ManualInputPrompt {
						message: "paste".into(),
						placeholder: server.redirect_uri.clone(),
					};
					let _ = result_sender
						.send(wait_for_callback_or_manual_input(&interaction, Some(&server), prompt).await);
				});
			}
			fetch("GET", &callback_url(&server.redirect_uri, &[("code", "from-browser")])).await;
			assert_eq!(
				result.await.unwrap().unwrap(),
				CallbackOrManualInput::Callback {
					value: "from-browser".into()
				}
			);
			assert!(manual_signal.borrow().as_ref().unwrap().aborted());
			server.close();
		});
	}

	#[test]
	fn returns_pasted_input_and_stops_waiting_for_the_browser() {
		pi_host::native::run(|| async {
			let server = start(plain_options()).await.unwrap();
			let result = wait_for_callback_or_manual_input(
				&interaction(|_| Box::pin(async { Ok("pasted".to_string()) }), None),
				Some(&server),
				ManualInputPrompt {
					message: "paste".into(),
					placeholder: server.redirect_uri.clone(),
				},
			)
			.await;
			assert_eq!(
				result.unwrap(),
				CallbackOrManualInput::Manual { input: "pasted".into() }
			);
			server.close();
		});
	}

	#[test]
	fn uses_only_the_manual_prompt_without_a_callback_server() {
		pi_host::native::run(|| async {
			let result = wait_for_callback_or_manual_input::<String>(
				&interaction(|_| Box::pin(async { Ok("pasted".to_string()) }), None),
				None,
				ManualInputPrompt {
					message: "paste".into(),
					placeholder: "http://localhost/callback".into(),
				},
			)
			.await;
			assert_eq!(
				result.unwrap(),
				CallbackOrManualInput::Manual { input: "pasted".into() }
			);
		});
	}

	#[test]
	fn propagates_manual_prompt_failures() {
		pi_host::native::run(|| async {
			let server = start(plain_options()).await.unwrap();
			let result = wait_for_callback_or_manual_input(
				&interaction(|_| Box::pin(async { Err(PiError::new("prompt cancelled")) }), None),
				Some(&server),
				ManualInputPrompt {
					message: "paste".into(),
					placeholder: server.redirect_uri.clone(),
				},
			)
			.await;
			assert!(result.unwrap_err().message.contains("prompt cancelled"));
			server.close();
		});
	}
}

/// Rust-only checks of the Node behaviours the port reproduces by hand.
mod node_behaviour {
	use futures::FutureExt;
	use tokio::io::{AsyncReadExt, AsyncWriteExt};

	use super::*;

	async fn raw_request(redirect_uri: &str, request: &[u8]) -> String {
		let url = Url::parse(redirect_uri).unwrap();
		let mut stream = tokio::net::TcpStream::connect(("127.0.0.1", url.port().unwrap()))
			.await
			.unwrap();
		stream.write_all(request).await.unwrap();
		let mut response = Vec::new();
		let _ = stream.read_to_end(&mut response).await;
		String::from_utf8_lossy(&response).into_owned()
	}

	#[test]
	fn close_frees_the_port_at_once() {
		pi_host::native::run(|| async {
			let server = start(options()).await.unwrap();
			let port = Url::parse(&server.redirect_uri).unwrap().port().unwrap();
			server.close();
			let again = start(OAuthCallbackServerOptions { port, ..options() }).await.unwrap();
			again.close();
		});
	}

	#[test]
	fn dropping_the_server_handle_keeps_the_wait_alive() {
		pi_host::native::run(|| async {
			let controller = AbortController::new();
			let server = start(OAuthCallbackServerOptions {
				signal: Some(controller.signal()),
				..options()
			})
			.await
			.unwrap();
			let wait = server.wait();
			drop(server);
			controller.abort();
			assert_eq!(wait.await.unwrap_err().message, "Login cancelled");
		});
	}

	#[test]
	fn rejects_requests_node_cannot_parse() {
		pi_host::native::run(|| async {
			let server = start(options()).await.unwrap();
			let lowercase = raw_request(&server.redirect_uri, b"get /callback HTTP/1.1\r\nHost: x\r\n\r\n").await;
			assert!(lowercase.starts_with("HTTP/1.1 400 Bad Request\r\n"), "{lowercase}");
			let bare_lf = raw_request(&server.redirect_uri, b"GET /callback HTTP/1.1\nHost: x\n\n").await;
			assert!(bare_lf.starts_with("HTTP/1.1 400 Bad Request\r\n"), "{bare_lf}");
			let non_ascii = raw_request(
				&server.redirect_uri,
				b"GET /callback?code=\xC3\xA9 HTTP/1.1\r\nHost: x\r\n\r\n",
			)
			.await;
			assert!(non_ascii.starts_with("HTTP/1.1 400 Bad Request\r\n"), "{non_ascii}");
			let mut large = b"GET /callback HTTP/1.1\r\nX-Large: ".to_vec();
			large.extend(std::iter::repeat_n(b'a', 20 * 1024));
			large.extend_from_slice(b"\r\n\r\n");
			let too_large = raw_request(&server.redirect_uri, &large).await;
			assert!(
				too_large.starts_with("HTTP/1.1 431 Request Header Fields Too Large\r\n"),
				"{too_large}"
			);
			server.close();
		});
	}

	#[test]
	fn dropping_the_wait_aborts_the_manual_prompt() {
		pi_host::native::run(|| async {
			let manual_signal: Rc<RefCell<Option<AbortSignal>>> = Rc::new(RefCell::new(None));
			let recorded = manual_signal.clone();
			let interaction = interaction(
				pending_prompt(move |prompt| *recorded.borrow_mut() = prompt.signal.clone()),
				None,
			);
			let mut waiting = Box::pin(wait_for_callback_or_manual_input::<String>(
				&interaction,
				None,
				ManualInputPrompt {
					message: "paste".into(),
					placeholder: "url".into(),
				},
			));
			assert!((&mut waiting).now_or_never().is_none());
			assert!(!manual_signal.borrow().as_ref().unwrap().aborted());
			drop(waiting);
			assert!(manual_signal.borrow().as_ref().unwrap().aborted());
		});
	}
}
