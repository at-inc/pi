//! Port of `packages/ai/test/anthropic-oauth.test.ts`.
//!
//! The browser-callback tests need the `native` feature (a real loopback socket). Tests that start the browser
//! login read `PI_OAUTH_CALLBACK_HOST` through `utils/provider_env.rs` (w2-ai-core) and wait for it.

mod common;

use std::cell::RefCell;
use std::rc::Rc;

use common::{json_body, json_response, run};
use pi_ai::auth::oauth::anthropic::anthropic_oauth;
use pi_ai::auth::types::{
	AuthEvent, AuthPrompt, AuthPromptKind, AuthSelectOption, OAuthCredential, ProviderAuthInteraction,
};
use pi_host::{AbortController, AbortSignal, HttpRequest, LocalBoxFuture, PiError, PiResult};
use serde_json::json;
use url::Url;

const TOKEN_URL: &str = "https://platform.claude.com/v1/oauth/token";

fn search_param(url: &str, name: &str) -> Option<String> {
	Url::parse(url)
		.unwrap()
		.query_pairs()
		.find(|(key, _)| key == name)
		.map(|(_, value)| value.into_owned())
}

fn interaction(
	notify: impl Fn(AuthEvent) + 'static,
	prompt: impl Fn(AuthPrompt) -> LocalBoxFuture<'static, PiResult<String>> + 'static,
) -> ProviderAuthInteraction {
	ProviderAuthInteraction {
		signal: AbortController::new().signal(),
		notify: Rc::new(notify),
		prompt: Rc::new(prompt),
	}
}

fn token_handler(check: impl Fn(&HttpRequest) + 'static, calls: Rc<RefCell<usize>>) -> common::FetchHandler {
	Rc::new(move |request| {
		*calls.borrow_mut() += 1;
		check(&request);
		Box::pin(async {
			Ok(json_response(
				json!({ "access_token": "access-token", "refresh_token": "refresh-token", "expires_in": 3600 }),
				200,
			))
		})
	})
}

mod anthropic_oauth_suite {
	use super::*;

	#[test]
	#[ignore = "PORT-STUB: needs utils/provider_env.rs from w2-ai-core"]
	fn keeps_the_localhost_redirect_uri_for_manual_callback_login() {
		let _sequential = common::sequential();
		let calls = Rc::new(RefCell::new(0));
		let handler = token_handler(
			|request| {
				assert_eq!(request.url, TOKEN_URL);
				assert_eq!(request.method, "POST");
				let body = json_body(request);
				assert_eq!(body["grant_type"], "authorization_code");
				assert_eq!(body["code"], "manual-code");
				assert_eq!(body["redirect_uri"], "http://localhost:53692/callback");
			},
			calls.clone(),
		);
		let auth_url = Rc::new(RefCell::new(String::new()));
		let credentials = run(handler, || {
			let notified = auth_url.clone();
			let prompted = auth_url.clone();
			anthropic_oauth().login(
				interaction(
					move |event| {
						if let AuthEvent::AuthUrl { url, .. } = event {
							*notified.borrow_mut() = url;
						}
					},
					move |prompt| {
						let auth_url = prompted.borrow().clone();
						Box::pin(async move {
							match &prompt.kind {
								AuthPromptKind::Select { .. } => return Ok("browser".into()),
								AuthPromptKind::ManualCode { .. } => {}
								_ => return Err(PiError::new(format!("Unexpected prompt: {}", prompt.type_name()))),
							}
							let state = search_param(&auth_url, "state");
							let redirect_uri = search_param(&auth_url, "redirect_uri");
							let (Some(state), Some(redirect_uri)) = (state, redirect_uri) else {
								return Err(PiError::new("Missing OAuth state or redirect_uri in auth URL"));
							};
							Ok(format!("{redirect_uri}?code=manual-code&state={state}"))
						})
					},
				),
				None,
			)
		})
		.unwrap();

		assert_eq!(credentials.access, "access-token");
		assert_eq!(credentials.refresh, "refresh-token");
		assert_eq!(*calls.borrow(), 1);
		assert_eq!(search_param(&auth_url.borrow(), "prompt").as_deref(), Some("login"));
	}

	#[test]
	fn offers_browser_login_first_and_uses_the_selected_anthropic_copy_code_flow() {
		let _sequential = common::sequential();
		let calls = Rc::new(RefCell::new(0));
		let auth_url = Rc::new(RefCell::new(String::new()));
		let checked = auth_url.clone();
		let handler = token_handler(
			move |request| {
				assert_eq!(request.url, TOKEN_URL);
				let body = json_body(request);
				assert_eq!(body["grant_type"], "authorization_code");
				assert_eq!(body["code"], "copied-code");
				assert_eq!(
					body["state"].as_str(),
					search_param(&checked.borrow(), "state").as_deref()
				);
				assert_eq!(body["redirect_uri"], "https://platform.claude.com/oauth/code/callback");
			},
			calls.clone(),
		);
		let select_prompts: Rc<RefCell<Vec<AuthPromptKind>>> = Rc::new(RefCell::new(Vec::new()));
		let credentials = run(handler, || {
			let notified = auth_url.clone();
			let prompted = auth_url.clone();
			let selects = select_prompts.clone();
			anthropic_oauth().login(
				interaction(
					move |event| {
						if let AuthEvent::AuthUrl { url, .. } = event {
							*notified.borrow_mut() = url;
						}
					},
					move |prompt| {
						let auth_url = prompted.borrow().clone();
						let selects = selects.clone();
						Box::pin(async move {
							match &prompt.kind {
								AuthPromptKind::Select { .. } => {
									selects.borrow_mut().push(prompt.kind.clone());
									Ok("copy_code".into())
								}
								AuthPromptKind::ManualCode { .. } => {
									Ok(format!("copied-code#{}", search_param(&auth_url, "state").unwrap()))
								}
								_ => Err(PiError::new(format!("Unexpected prompt: {}", prompt.type_name()))),
							}
						})
					},
				),
				None,
			)
		})
		.unwrap();

		assert_eq!(credentials.access, "access-token");
		assert_eq!(credentials.refresh, "refresh-token");
		assert_eq!(
			search_param(&auth_url.borrow(), "redirect_uri").as_deref(),
			Some("https://platform.claude.com/oauth/code/callback")
		);
		assert_eq!(*calls.borrow(), 1);
		assert_eq!(
			*select_prompts.borrow(),
			vec![AuthPromptKind::Select {
				message: "Select Anthropic login method:".into(),
				options: vec![
					AuthSelectOption::new("browser", "Browser login (default)"),
					AuthSelectOption::new("copy_code", "Copy code login (headless)"),
				],
			}]
		);
	}

	#[test]
	fn cancels_when_anthropic_login_method_selection_is_cancelled() {
		let _sequential = common::sequential();
		let handler: common::FetchHandler = Rc::new(|_| Box::pin(async { Err(PiError::new("unexpected fetch")) }));
		let error = run(handler, || {
			anthropic_oauth().login(
				interaction(|_| {}, |_| Box::pin(async { Err(PiError::new("Login cancelled")) })),
				None,
			)
		})
		.unwrap_err();
		assert!(error.message.contains("Login cancelled"));
	}

	#[test]
	fn omits_scope_from_refresh_token_requests() {
		let _sequential = common::sequential();
		let calls = Rc::new(RefCell::new(0));
		let handler: common::FetchHandler = {
			let calls = calls.clone();
			Rc::new(move |request| {
				*calls.borrow_mut() += 1;
				assert_eq!(request.url, TOKEN_URL);
				assert_eq!(request.method, "POST");
				let body = json_body(&request);
				assert_eq!(body["grant_type"], "refresh_token");
				assert!(body["client_id"].as_str().is_some_and(|id| !id.is_empty()));
				assert_eq!(body["refresh_token"], "refresh-token");
				assert!(body.get("scope").is_none());
				Box::pin(async {
					Ok(json_response(
						json!({ "access_token": "new-access-token", "refresh_token": "new-refresh-token", "expires_in": 3600 }),
						200,
					))
				})
			})
		};
		let credentials = run(handler, || {
			anthropic_oauth().refresh(
				OAuthCredential::new("refresh-token", "old-access-token", 0.0),
				AbortController::new().signal(),
			)
		})
		.unwrap();

		assert_eq!(credentials.access, "new-access-token");
		assert_eq!(credentials.refresh, "new-refresh-token");
		assert_eq!(*calls.borrow(), 1);
	}

	#[test]
	#[ignore = "PORT-STUB: needs utils/provider_env.rs from w2-ai-core"]
	fn anthropic_oauth_login_resolves_through_the_manual_code_prompt_and_aborts_it_after_settling() {
		let _sequential = common::sequential();
		let handler: common::FetchHandler = Rc::new(|request| {
			let url = request.url.clone();
			Box::pin(async move {
				if url.contains("/oauth/token") {
					return Ok(json_response(
						json!({ "access_token": "access", "refresh_token": "refresh", "expires_in": 3600 }),
						200,
					));
				}
				Err(PiError::new(format!("Unexpected fetch: {url}")))
			})
		});
		let events: Rc<RefCell<Vec<AuthEvent>>> = Rc::new(RefCell::new(Vec::new()));
		let prompts: Rc<RefCell<Vec<&'static str>>> = Rc::new(RefCell::new(Vec::new()));
		let manual_signal: Rc<RefCell<Option<AbortSignal>>> = Rc::new(RefCell::new(None));
		let credential = run(handler, || {
			let events = events.clone();
			let prompts = prompts.clone();
			let manual_signal = manual_signal.clone();
			anthropic_oauth().login(
				interaction(
					move |event| events.borrow_mut().push(event),
					move |prompt| {
						prompts.borrow_mut().push(prompt.type_name());
						let result = match &prompt.kind {
							AuthPromptKind::Select { .. } => Ok("browser".to_string()),
							AuthPromptKind::ManualCode { .. } => {
								*manual_signal.borrow_mut() = prompt.signal.clone();
								Ok("the-code".to_string())
							}
							_ => Err(PiError::new(format!("Unexpected prompt: {}", prompt.type_name()))),
						};
						Box::pin(async move { result })
					},
				),
				None,
			)
		})
		.unwrap();

		assert_eq!(credential.access, "access");
		assert!(
			events
				.borrow()
				.iter()
				.any(|event| matches!(event, AuthEvent::AuthUrl { .. }))
		);
		assert!(prompts.borrow().contains(&"manual_code"));
		// the prompt's signal is aborted once login settles, so UIs can dismiss it
		assert!(manual_signal.borrow().as_ref().unwrap().aborted());
	}

	#[cfg(feature = "native")]
	mod browser_callback {
		use super::*;

		struct BrowserLogin {
			credential: OAuthCredential,
			redirect_uri: String,
			exchanged_code: Option<String>,
			exchanged_redirect_uri: Option<String>,
			page_status: u16,
			page_text: String,
		}

		fn login_through_browser_callback() -> BrowserLogin {
			let exchanged: Rc<RefCell<(Option<String>, Option<String>)>> = Rc::new(RefCell::new((None, None)));
			let recorded = exchanged.clone();
			let handler: common::FetchHandler = Rc::new(move |request| {
				if request.url != TOKEN_URL {
					return common::native_fetch(request);
				}
				let body = json_body(&request);
				*recorded.borrow_mut() = (
					body["code"].as_str().map(str::to_string),
					body["redirect_uri"].as_str().map(str::to_string),
				);
				Box::pin(async {
					Ok(json_response(
						json!({ "access_token": "access", "refresh_token": "refresh", "expires_in": 3600 }),
						200,
					))
				})
			});
			let redirect_uri = Rc::new(RefCell::new(String::new()));
			let (page_sender, page) = futures::channel::oneshot::channel();
			let page_sender = Rc::new(RefCell::new(Some(page_sender)));
			let (credential, page) = run(handler, || {
				let redirect = redirect_uri.clone();
				async move {
					let credential = anthropic_oauth()
						.login(
							interaction(
								move |event| {
									let AuthEvent::AuthUrl { url, .. } = event else {
										return;
									};
									let state = search_param(&url, "state").unwrap_or_default();
									let uri = search_param(&url, "redirect_uri").unwrap_or_default();
									*redirect.borrow_mut() = uri.clone();
									let mut callback_url = Url::parse(&uri).unwrap();
									callback_url.set_host(Some("127.0.0.1")).unwrap();
									callback_url
										.query_pairs_mut()
										.clear()
										.append_pair("code", "browser-code")
										.append_pair("state", &state);
									let sender = page_sender.borrow_mut().take();
									pi_host::spawn_local(async move {
										let response =
											common::native_fetch(HttpRequest::new("GET", callback_url.as_str()))
												.await
												.unwrap();
										let status = response.status;
										let text = response.text().await.unwrap();
										if let Some(sender) = sender {
											let _ = sender.send((status, text));
										}
									});
								},
								|prompt| {
									if matches!(prompt.kind, AuthPromptKind::Select { .. }) {
										return Box::pin(async { Ok("browser".to_string()) });
									}
									let signal = prompt.signal.clone().unwrap();
									Box::pin(async move {
										signal.aborted_future().await;
										Err(PiError::new("aborted"))
									})
								},
							),
							None,
						)
						.await
						.unwrap();
					(credential, page.await.unwrap())
				}
			});
			let (exchanged_code, exchanged_redirect_uri) = exchanged.borrow().clone();
			BrowserLogin {
				credential,
				redirect_uri: redirect_uri.borrow().clone(),
				exchanged_code,
				exchanged_redirect_uri,
				page_status: page.0,
				page_text: page.1,
			}
		}

		#[test]
		#[ignore = "PORT-STUB: needs utils/provider_env.rs and utils/oauth_page.rs from w2-ai-core"]
		fn completes_login_through_the_browser_callback_and_shows_the_sign_in_page() {
			let _sequential = common::sequential();
			let login = login_through_browser_callback();

			assert_eq!(login.credential.access, "access");
			assert_eq!(login.exchanged_code.as_deref(), Some("browser-code"));
			assert_eq!(
				login.exchanged_redirect_uri.as_deref(),
				Some(login.redirect_uri.as_str())
			);
			assert_eq!(login.page_status, 200);
			assert!(login.page_text.contains("Signed in to Anthropic."));
		}

		// #10571
		#[test]
		#[ignore = "PORT-STUB: needs utils/provider_env.rs and utils/oauth_page.rs from w2-ai-core"]
		fn falls_back_to_a_free_callback_port_when_the_preferred_port_cannot_be_bound() {
			let _sequential = common::sequential();
			let blocker = std::net::TcpListener::bind("127.0.0.1:53692").unwrap();
			let login = login_through_browser_callback();
			drop(blocker);
			let redirect_uri = Url::parse(&login.redirect_uri).unwrap();

			assert_eq!(redirect_uri.host_str(), Some("localhost"));
			assert_eq!(redirect_uri.path(), "/callback");
			assert_ne!(redirect_uri.port(), Some(53692));
			assert_eq!(login.credential.access, "access");
			assert_eq!(
				login.exchanged_redirect_uri.as_deref(),
				Some(login.redirect_uri.as_str())
			);
			assert_eq!(login.page_status, 200);
		}
	}
}
