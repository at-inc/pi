//! Port of `packages/ai/test/openrouter-oauth.test.ts`.
//!
//! Needs the `native` feature: login starts the loopback callback server, and the tests reach it with the platform
//! `fetch`. Every login reads `PI_OAUTH_CALLBACK_HOST` through `utils/provider_env.rs` (w2-ai-core) and waits for
//! it; the success page also needs `utils/oauth_page.rs`. The first two TS cases (`openrouterProvider()` exposing
//! the flow, and `createModels` resolving the stored key for chat and image models) exercise `models.ts` and
//! `providers/openrouter.ts` of shard w2-ai-core and are not ported here.

#![cfg(feature = "native")]

mod common;

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use common::{FetchHandler, json_body, json_response, native_fetch, sequential};
use futures::channel::oneshot;
use pi_ai::auth::oauth::openrouter::open_router_oauth;
use pi_ai::auth::types::{AuthEvent, AuthPrompt, AuthPromptKind, OAuthCredential, ProviderAuthInteraction};
use pi_host::{AbortController, AbortSignal, HttpRequest, HttpResponse, LocalBoxFuture, PiError, PiResult};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use url::Url;

const TOKEN_URL: &str = "https://openrouter.ai/api/v1/auth/keys";

fn search_param(url: &Url, name: &str) -> Option<String> {
	url.query_pairs()
		.find(|(key, _)| key == name)
		.map(|(_, value)| value.into_owned())
}

fn callback_url_of(auth_url: &str) -> Url {
	Url::parse(&search_param(&Url::parse(auth_url).unwrap(), "callback_url").unwrap_or_default()).unwrap()
}

fn with_code(mut url: Url, code: &str) -> Url {
	url.query_pairs_mut().append_pair("code", code);
	url
}

/// Starts `nativeFetch(url)` and resolves with the response status.
fn fetch_status(url: Url) -> oneshot::Receiver<PiResult<u16>> {
	let (sender, receiver) = oneshot::channel();
	pi_host::spawn_local(async move {
		let response = native_fetch(HttpRequest::new("GET", url.as_str())).await;
		let _ = sender.send(response.map(|response| response.status));
	});
	receiver
}

fn never_answers() -> impl Fn(AuthPrompt) -> LocalBoxFuture<'static, PiResult<String>> + 'static {
	|_| Box::pin(futures::future::pending())
}

fn interaction(
	signal: Option<AbortSignal>,
	prompt: impl Fn(AuthPrompt) -> LocalBoxFuture<'static, PiResult<String>> + 'static,
	notify: impl Fn(AuthEvent) + 'static,
) -> ProviderAuthInteraction {
	ProviderAuthInteraction {
		signal: signal.unwrap_or_else(|| AbortController::new().signal()),
		prompt: Rc::new(prompt),
		notify: Rc::new(notify),
	}
}

/// A `fetch` stub that answers `TOKEN_URL` with `respond`, records the exchange body, and passes other requests
/// on to the platform `fetch`; `calls` counts the stubbed requests.
fn token_endpoint(
	respond: impl Fn() -> HttpResponse + 'static,
	exchange_body: Rc<RefCell<Option<Value>>>,
	calls: Rc<Cell<usize>>,
) -> FetchHandler {
	Rc::new(move |request: HttpRequest| {
		if request.url != TOKEN_URL {
			return native_fetch(request);
		}
		calls.set(calls.get() + 1);
		*exchange_body.borrow_mut() = Some(json_body(&request));
		let response = respond();
		Box::pin(async move { Ok(response) })
	})
}

fn on_auth_url(handle: impl Fn(&str) + 'static) -> impl Fn(AuthEvent) + 'static {
	move |event| {
		if let AuthEvent::AuthUrl { url, .. } = event {
			handle(&url);
		}
	}
}

fn credential_json(credential: &OAuthCredential) -> Value {
	serde_json::to_value(credential).unwrap()
}

mod open_router_oauth {
	use super::*;

	#[test]
	#[ignore = "PORT-STUB: needs utils/provider_env.rs and utils/oauth_page.rs from w2-ai-core"]
	fn runs_pkce_on_a_one_shot_loopback_callback_and_exchanges_the_code_for_a_permanent_api_key() {
		let _sequential = sequential();
		let exchange_body = Rc::new(RefCell::new(None));
		let calls = Rc::new(Cell::new(0));
		let fetch = token_endpoint(
			|| json_response(json!({ "key": "sk-or-test" }), 200),
			exchange_body.clone(),
			calls.clone(),
		);
		common::run(fetch, || async {
			let authorize_url: Rc<RefCell<Option<Url>>> = Rc::new(RefCell::new(None));
			let callback_response: Rc<RefCell<Option<oneshot::Receiver<PiResult<u16>>>>> = Rc::new(RefCell::new(None));
			let manual_signal: Rc<RefCell<Option<AbortSignal>>> = Rc::new(RefCell::new(None));
			let (recorded_url, recorded_response, recorded_signal) =
				(authorize_url.clone(), callback_response.clone(), manual_signal.clone());
			let credential = open_router_oauth()
				.login(
					interaction(
						None,
						move |prompt| {
							*recorded_signal.borrow_mut() = prompt.signal.clone();
							Box::pin(futures::future::pending())
						},
						on_auth_url(move |url| {
							*recorded_url.borrow_mut() = Some(Url::parse(url).unwrap());
							*recorded_response.borrow_mut() =
								Some(fetch_status(with_code(callback_url_of(url), "authorization-code")));
						}),
					),
					None,
				)
				.await
				.unwrap();

			assert_eq!(
				credential_json(&credential),
				json!({ "type": "oauth", "refresh": "", "access": "sk-or-test", "expires": 9_007_199_254_740_991.0 })
			);
			let callback_response = callback_response.borrow_mut().take().unwrap();
			assert_eq!(callback_response.await.unwrap().unwrap(), 200);
			assert!(manual_signal.borrow().as_ref().unwrap().aborted());
			let authorize_url = authorize_url.borrow().clone().unwrap();
			assert_eq!(authorize_url.origin().ascii_serialization(), "https://openrouter.ai");
			assert_eq!(authorize_url.path(), "/auth");
			assert_eq!(
				search_param(&authorize_url, "code_challenge_method").as_deref(),
				Some("S256")
			);

			let callback_url = callback_url_of(authorize_url.as_str());
			assert_eq!(callback_url.host_str(), Some("127.0.0.1"));
			assert!(
				regex::Regex::new(r"^/oauth/callback/[0-9a-f-]+$")
					.unwrap()
					.is_match(callback_url.path())
			);

			let exchange_body = exchange_body.borrow().clone().unwrap();
			assert_eq!(exchange_body["code"], "authorization-code");
			assert_eq!(exchange_body["code_challenge_method"], "S256");
			let verifier = exchange_body["code_verifier"].as_str().expect("verifier is a string");
			assert_eq!(
				search_param(&authorize_url, "code_challenge"),
				Some(URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes())))
			);
			assert_eq!(calls.get(), 1);
		});
	}

	#[test]
	#[ignore = "PORT-STUB: needs utils/provider_env.rs and utils/oauth_page.rs from w2-ai-core"]
	fn reports_token_exchange_failures_through_both_the_callback_page_and_login() {
		let _sequential = sequential();
		let fetch = token_endpoint(
			|| json_response(json!({ "error": { "message": "invalid code" } }), 403),
			Rc::new(RefCell::new(None)),
			Rc::new(Cell::new(0)),
		);
		common::run(fetch, || async {
			let callback_response: Rc<RefCell<Option<oneshot::Receiver<PiResult<u16>>>>> = Rc::new(RefCell::new(None));
			let recorded = callback_response.clone();
			let error = open_router_oauth()
				.login(
					interaction(
						None,
						never_answers(),
						on_auth_url(move |url| {
							*recorded.borrow_mut() = Some(fetch_status(with_code(callback_url_of(url), "bad-code")));
						}),
					),
					None,
				)
				.await
				.unwrap_err();
			assert!(
				error
					.message
					.contains("OpenRouter OAuth key exchange failed (HTTP 403): invalid code")
			);
			let callback_response = callback_response.borrow_mut().take().unwrap();
			assert_eq!(callback_response.await.unwrap().unwrap(), 502);
		});
	}

	#[test]
	#[ignore = "PORT-STUB: needs utils/provider_env.rs and utils/oauth_page.rs from w2-ai-core"]
	fn allows_only_one_token_exchange_for_a_callback() {
		let _sequential = sequential();
		let complete_exchange: Rc<RefCell<Option<oneshot::Sender<HttpResponse>>>> = Rc::new(RefCell::new(None));
		let calls = Rc::new(Cell::new(0));
		let fetch: FetchHandler = {
			let complete_exchange = complete_exchange.clone();
			let calls = calls.clone();
			Rc::new(move |request: HttpRequest| {
				if request.url != TOKEN_URL {
					return native_fetch(request);
				}
				calls.set(calls.get() + 1);
				let (sender, receiver) = oneshot::channel();
				*complete_exchange.borrow_mut() = Some(sender);
				Box::pin(async move { receiver.await.map_err(|_| PiError::new("Token exchange did not start")) })
			})
		};
		common::run(fetch, || async {
			let callback_url: Rc<RefCell<Option<Url>>> = Rc::new(RefCell::new(None));
			let first_callback: Rc<RefCell<Option<oneshot::Receiver<PiResult<u16>>>>> = Rc::new(RefCell::new(None));
			let (recorded_url, recorded_callback) = (callback_url.clone(), first_callback.clone());
			let (login_sender, login) = oneshot::channel();
			pi_host::spawn_local(async move {
				let result = open_router_oauth()
					.login(
						interaction(
							None,
							never_answers(),
							on_auth_url(move |url| {
								let url = with_code(callback_url_of(url), "authorization-code");
								*recorded_url.borrow_mut() = Some(url.clone());
								*recorded_callback.borrow_mut() = Some(fetch_status(url));
							}),
						),
						None,
					)
					.await;
				let _ = login_sender.send(result);
			});

			while calls.get() < 1 {
				pi_host::sleep(5.0, None).await.unwrap();
			}
			let callback_url = callback_url
				.borrow()
				.clone()
				.expect("OpenRouter did not provide a callback URL");
			let second = native_fetch(HttpRequest::new("GET", callback_url.as_str()))
				.await
				.unwrap();
			assert_eq!(second.status, 409);
			assert_eq!(calls.get(), 1);
			let sender = complete_exchange.borrow_mut().take().unwrap();
			let _ = sender.send(json_response(json!({ "key": "sk-or-test" }), 200));

			assert_eq!(login.await.unwrap().unwrap().access, "sk-or-test");
			let first_callback = first_callback.borrow_mut().take().unwrap();
			assert_eq!(first_callback.await.unwrap().unwrap(), 200);
		});
	}

	#[test]
	#[ignore = "PORT-STUB: needs utils/provider_env.rs and utils/oauth_page.rs from w2-ai-core"]
	fn rejects_a_successful_response_that_does_not_contain_a_key() {
		let _sequential = sequential();
		let fetch = token_endpoint(
			|| json_response(json!({ "user_id": "user-1" }), 200),
			Rc::new(RefCell::new(None)),
			Rc::new(Cell::new(0)),
		);
		common::run(fetch, || async {
			let callback_response: Rc<RefCell<Option<oneshot::Receiver<PiResult<u16>>>>> = Rc::new(RefCell::new(None));
			let recorded = callback_response.clone();
			let error = open_router_oauth()
				.login(
					interaction(
						None,
						never_answers(),
						on_auth_url(move |url| {
							*recorded.borrow_mut() =
								Some(fetch_status(with_code(callback_url_of(url), "code-without-key")));
						}),
					),
					None,
				)
				.await
				.unwrap_err();
			assert!(error.message.contains("OpenRouter OAuth response carries no \"key\""));
			let callback_response = callback_response.borrow_mut().take().unwrap();
			assert_eq!(callback_response.await.unwrap().unwrap(), 502);
		});
	}

	#[test]
	#[ignore = "PORT-STUB: needs utils/provider_env.rs from w2-ai-core"]
	fn mints_a_key_from_a_pasted_redirect_url_when_the_loopback_callback_never_arrives() {
		let _sequential = sequential();
		let exchange_body = Rc::new(RefCell::new(None));
		let calls = Rc::new(Cell::new(0));
		let fetch = token_endpoint(
			|| json_response(json!({ "key": "sk-or-manual" }), 200),
			exchange_body.clone(),
			calls.clone(),
		);
		common::run(fetch, || async {
			let callback_url: Rc<RefCell<Option<String>>> = Rc::new(RefCell::new(None));
			let (read_url, recorded_url) = (callback_url.clone(), callback_url.clone());
			let credential = open_router_oauth()
				.login(
					interaction(
						None,
						move |prompt| {
							let url = read_url.borrow().clone();
							Box::pin(async move {
								if !matches!(prompt.kind, AuthPromptKind::ManualCode { .. }) {
									return Err(PiError::new(format!("Unexpected prompt: {}", prompt.type_name())));
								}
								Ok(format!("{}?code=manual-code", url.unwrap_or_default()))
							})
						},
						on_auth_url(move |url| {
							*recorded_url.borrow_mut() = search_param(&Url::parse(url).unwrap(), "callback_url");
						}),
					),
					None,
				)
				.await
				.unwrap();
			assert_eq!(
				credential_json(&credential),
				json!({ "type": "oauth", "refresh": "", "access": "sk-or-manual", "expires": 9_007_199_254_740_991.0 })
			);
			let exchange_body = exchange_body.borrow().clone().unwrap();
			assert_eq!(exchange_body["code"], "manual-code");
			assert_eq!(exchange_body["code_challenge_method"], "S256");
			assert_eq!(calls.get(), 1);
		});
	}

	#[test]
	#[ignore = "PORT-STUB: needs utils/provider_env.rs from w2-ai-core"]
	fn accepts_a_bare_authorization_code_from_the_manual_prompt() {
		let _sequential = sequential();
		let exchange_body = Rc::new(RefCell::new(None));
		let fetch = token_endpoint(
			|| json_response(json!({ "key": "sk-or-manual" }), 200),
			exchange_body.clone(),
			Rc::new(Cell::new(0)),
		);
		common::run(fetch, || async {
			let credential = open_router_oauth()
				.login(
					interaction(None, |_| Box::pin(async { Ok("  manual-code  ".to_string()) }), |_| {}),
					None,
				)
				.await
				.unwrap();
			assert_eq!(credential.access, "sk-or-manual");
			assert_eq!(exchange_body.borrow().as_ref().unwrap()["code"], "manual-code");
		});
	}

	#[test]
	#[ignore = "PORT-STUB: needs utils/provider_env.rs from w2-ai-core"]
	fn fails_login_when_the_manual_prompt_is_cancelled() {
		let _sequential = sequential();
		let calls = Rc::new(Cell::new(0));
		let fetch = token_endpoint(
			|| json_response(json!({ "key": "sk-or-unexpected" }), 200),
			Rc::new(RefCell::new(None)),
			calls.clone(),
		);
		common::run(fetch, || async {
			let error = open_router_oauth()
				.login(
					interaction(
						None,
						|_| Box::pin(async { Err(PiError::new("Login cancelled")) }),
						|_| {},
					),
					None,
				)
				.await
				.unwrap_err();
			assert!(error.message.contains("Login cancelled"));
			assert_eq!(calls.get(), 0);
		});
	}

	#[test]
	#[ignore = "PORT-STUB: needs utils/provider_env.rs from w2-ai-core"]
	fn rejects_empty_manual_input_without_exchanging_a_code() {
		let _sequential = sequential();
		let calls = Rc::new(Cell::new(0));
		let fetch = token_endpoint(
			|| json_response(json!({ "key": "sk-or-unexpected" }), 200),
			Rc::new(RefCell::new(None)),
			calls.clone(),
		);
		common::run(fetch, || async {
			let error = open_router_oauth()
				.login(
					interaction(None, |_| Box::pin(async { Ok("   ".to_string()) }), |_| {}),
					None,
				)
				.await
				.unwrap_err();
			assert!(error.message.contains("Missing authorization code"));
			assert_eq!(calls.get(), 0);
		});
	}

	#[test]
	#[ignore = "PORT-STUB: needs utils/provider_env.rs from w2-ai-core"]
	fn closes_the_pending_callback_when_login_is_cancelled() {
		let _sequential = sequential();
		let fetch: FetchHandler = Rc::new(native_fetch);
		common::run(fetch, || async {
			let controller = AbortController::new();
			let callback_url: Rc<RefCell<Option<Url>>> = Rc::new(RefCell::new(None));
			let (recorded, aborter) = (callback_url.clone(), controller.clone());
			let error = open_router_oauth()
				.login(
					interaction(
						Some(controller.signal()),
						never_answers(),
						on_auth_url(move |url| {
							*recorded.borrow_mut() = Some(callback_url_of(url));
							aborter.abort();
						}),
					),
					None,
				)
				.await
				.unwrap_err();
			assert!(error.message.contains("Login cancelled"));
			let callback_url = callback_url.borrow().clone().expect("callback URL was announced");
			assert!(
				native_fetch(HttpRequest::new("GET", callback_url.as_str()))
					.await
					.is_err()
			);
		});
	}

	#[test]
	#[ignore = "PORT-STUB: needs utils/provider_env.rs from w2-ai-core"]
	fn rejects_before_opening_a_callback_server_when_login_is_already_cancelled() {
		let fetch: FetchHandler = Rc::new(native_fetch);
		common::run(fetch, || async {
			let controller = AbortController::new();
			controller.abort();
			let error = open_router_oauth()
				.login(
					interaction(
						Some(controller.signal()),
						|_| Box::pin(async { Ok(String::new()) }),
						|_| panic!("Cancelled login must not emit events"),
					),
					None,
				)
				.await
				.unwrap_err();
			assert!(error.message.contains("Login cancelled"));
		});
	}

	#[test]
	#[ignore = "PORT-STUB: needs utils/provider_env.rs from w2-ai-core"]
	fn uses_the_configured_oauth_callback_host() {
		let _sequential = sequential();
		let fetch: FetchHandler = Rc::new(native_fetch);
		common::run_with_env(fetch, &[("PI_OAUTH_CALLBACK_HOST", "localhost")], || async {
			let controller = AbortController::new();
			let callback_url: Rc<RefCell<Option<Url>>> = Rc::new(RefCell::new(None));
			let (recorded, aborter) = (callback_url.clone(), controller.clone());
			let error = open_router_oauth()
				.login(
					interaction(
						Some(controller.signal()),
						never_answers(),
						on_auth_url(move |url| {
							*recorded.borrow_mut() = Some(callback_url_of(url));
							aborter.abort();
						}),
					),
					None,
				)
				.await
				.unwrap_err();
			assert!(error.message.contains("Login cancelled"));
			assert_eq!(callback_url.borrow().as_ref().unwrap().host_str(), Some("localhost"));
		});
	}
}
