//! Port of `packages/ai/test/radius-oauth.test.ts`.
//!
//! The tests run on `pi_host::testing`'s fake timers, except the browser callback test, which needs the `native`
//! feature for the loopback callback server and reaches it with the platform `fetch`. `createRadiusOAuth` normalizes
//! the gateway through `providers/radius_config.rs` (w2-ai-core), so every test waits for it; the callback page also
//! needs `utils/oauth_page.rs`.

mod common;

use std::cell::RefCell;
use std::rc::Rc;

use common::{FetchHandler, form_body, json_response};
use pi_ai::auth::oauth::radius::{RadiusOAuthOptions, create_radius_oauth};
use pi_ai::auth::types::{AuthEvent, OAuthAuth, OAuthCredential, ProviderAuthInteraction};
use pi_host::{AbortController, HttpRequest, HttpResponse, PiError, PiResult};
use serde_json::{Value, json};

const GATEWAY: &str = "https://radius.example";

fn handler(respond: impl Fn(HttpRequest) -> PiResult<HttpResponse> + 'static) -> FetchHandler {
	Rc::new(move |request| {
		let response = respond(request);
		Box::pin(async move { response })
	})
}

fn interaction(login_method: &'static str, events: Rc<RefCell<Vec<Value>>>) -> ProviderAuthInteraction {
	ProviderAuthInteraction {
		signal: AbortController::new().signal(),
		prompt: Rc::new(move |_| Box::pin(async move { Ok(login_method.to_string()) })),
		notify: Rc::new(move |event: AuthEvent| events.borrow_mut().push(serde_json::to_value(event).unwrap())),
	}
}

fn radius_oauth() -> OAuthAuth {
	create_radius_oauth(RadiusOAuthOptions {
		name: "Radius".into(),
		gateway: GATEWAY.into(),
	})
}

fn credential_json(credential: &OAuthCredential) -> Value {
	serde_json::to_value(credential).unwrap()
}

mod radius_o_auth {
	use super::*;

	#[test]
	#[ignore = "PORT-STUB: needs providers/radius_config.rs from w2-ai-core"]
	fn uses_gateway_endpoints_directly_for_device_login() {
		let urls = Rc::new(RefCell::new(Vec::<String>::new()));
		let recorded = urls.clone();
		let fetch = handler(move |request| {
			let url = request.url.clone();
			recorded.borrow_mut().push(url.clone());
			let form = form_body(&request);
			if url == format!("{GATEWAY}/v1/oauth/device") {
				assert_eq!(form["client_id"], "pi-gateway");
				assert_eq!(form["scope"], "gateway offline_access");
				return Ok(json_response(
					json!({
						"device_code": "device-code",
						"user_code": "ABCD-1234",
						"verification_uri": "https://radius-ui.example/pair",
						"expires_in": 600,
						"interval": 5,
					}),
					200,
				));
			}
			if url == format!("{GATEWAY}/v1/oauth/token") {
				assert_eq!(form["grant_type"], "urn:ietf:params:oauth:grant-type:device_code");
				assert_eq!(form["client_id"], "pi-gateway");
				assert_eq!(form["device_code"], "device-code");
				return Ok(json_response(
					json!({
						"access_token": "access-token",
						"refresh_token": "refresh-token",
						"expires_in": 3600,
						"scope": "gateway offline_access",
					}),
					200,
				));
			}
			Err(PiError::new(format!("Unexpected request: {url}")))
		});

		let events = Rc::new(RefCell::new(Vec::new()));
		let (credential, now) = common::run_with_fake_timers(fetch, || async {
			let credential = radius_oauth()
				.login(interaction("device-code", events.clone()), None)
				.await;
			(credential, pi_host::now_ms())
		});
		assert_eq!(
			credential_json(&credential.unwrap()),
			json!({
				"type": "oauth",
				"refresh": "refresh-token",
				"access": "access-token",
				"expires": now + 3600.0 * 1000.0 - 60_000.0,
				"scope": "gateway offline_access",
			})
		);
		assert_eq!(
			*events.borrow(),
			vec![json!({
				"type": "device_code",
				"userCode": "ABCD-1234",
				"verificationUri": "https://radius-ui.example/pair",
				"intervalSeconds": 5.0,
				"expiresInSeconds": 600.0,
			})]
		);
		assert_eq!(
			*urls.borrow(),
			vec![
				format!("{GATEWAY}/v1/oauth/device"),
				format!("{GATEWAY}/v1/oauth/token")
			]
		);
	}

	#[test]
	#[ignore = "PORT-STUB: needs providers/radius_config.rs from w2-ai-core"]
	fn refreshes_directly_through_the_gateway_without_discovery() {
		let calls = Rc::new(RefCell::new(0));
		let count = calls.clone();
		let fetch = handler(move |request| {
			*count.borrow_mut() += 1;
			assert_eq!(request.url, format!("{GATEWAY}/v1/oauth/token"));
			let form = form_body(&request);
			assert_eq!(form["grant_type"], "refresh_token");
			assert_eq!(form["client_id"], "pi-gateway");
			assert_eq!(form["refresh_token"], "old-refresh");
			Ok(json_response(
				json!({ "access_token": "new-access", "refresh_token": "new-refresh", "expires_in": 3600 }),
				200,
			))
		});

		let credential = common::run_with_fake_timers(fetch, || {
			radius_oauth().refresh(
				OAuthCredential::new("old-refresh", "old-access", 0.0),
				AbortController::new().signal(),
			)
		})
		.unwrap();
		assert_eq!(credential.access, "new-access");
		assert_eq!(credential.refresh, "new-refresh");
		assert_eq!(*calls.borrow(), 1);
	}

	#[test]
	#[ignore = "PORT-STUB: needs providers/radius_config.rs from w2-ai-core"]
	fn discovers_only_the_interactive_browser_authorization_endpoint() {
		let calls = Rc::new(RefCell::new(0));
		let count = calls.clone();
		let fetch = handler(move |request| {
			*count.borrow_mut() += 1;
			assert_eq!(request.url, format!("{GATEWAY}/v1/oauth"));
			Ok(json_response(json!({ "issuer": "https://radius-ui.example" }), 200))
		});

		let error = common::run_with_fake_timers(fetch, || {
			radius_oauth().login(interaction("browser", Rc::default()), None)
		})
		.unwrap_err();
		assert!(
			error
				.message
				.contains(&format!("Invalid Radius OAuth config from {GATEWAY}")),
			"{}",
			error.message
		);
		assert_eq!(*calls.borrow(), 1);
	}

	#[test]
	#[ignore = "PORT-STUB: needs providers/radius_config.rs from w2-ai-core"]
	fn rejects_an_authorization_endpoint_that_is_not_an_http_s_url() {
		let fetch = handler(|_| {
			Ok(json_response(
				json!({ "authorizationEndpoint": "file:///Users/victim/Downloads/payload.app" }),
				200,
			))
		});
		let events = Rc::new(RefCell::new(Vec::new()));

		let error = common::run_with_fake_timers(fetch, || {
			radius_oauth().login(interaction("browser", events.clone()), None)
		})
		.unwrap_err();
		assert!(
			error
				.message
				.contains(&format!("Invalid Radius OAuth config from {GATEWAY}")),
			"{}",
			error.message
		);
		assert!(events.borrow().is_empty());
	}

	#[test]
	#[ignore = "PORT-STUB: needs providers/radius_config.rs from w2-ai-core"]
	fn rejects_a_device_verification_uri_that_is_not_an_http_s_url() {
		let fetch = handler(|_| {
			Ok(json_response(
				json!({
					"device_code": "device-code",
					"user_code": "ABCD-1234",
					"verification_uri": "file:///Users/victim/Downloads/payload.app",
					"expires_in": 600,
				}),
				200,
			))
		});
		let events = Rc::new(RefCell::new(Vec::new()));

		let error = common::run_with_fake_timers(fetch, || {
			radius_oauth().login(interaction("device-code", events.clone()), None)
		})
		.unwrap_err();
		assert!(
			error.message.contains("Untrusted verification_uri"),
			"{}",
			error.message
		);
		assert!(events.borrow().is_empty());
	}

	#[cfg(feature = "native")]
	#[test]
	#[ignore = "PORT-STUB: needs providers/radius_config.rs and utils/oauth_page.rs from w2-ai-core"]
	fn exchanges_the_browser_callback_code_before_showing_the_sign_in_page() {
		use std::cell::Cell;

		use futures::channel::oneshot;

		type CallbackPage = oneshot::Receiver<PiResult<(u16, String)>>;

		let _sequential = common::sequential();
		let token_status = Rc::new(Cell::new(400));
		let status = token_status.clone();
		let fetch: FetchHandler = Rc::new(move |request: HttpRequest| {
			let url = request.url.clone();
			if url == format!("{GATEWAY}/v1/oauth") {
				return Box::pin(async {
					Ok(json_response(
						json!({ "authorizationEndpoint": format!("{GATEWAY}/authorize") }),
						200,
					))
				});
			}
			if url == format!("{GATEWAY}/v1/oauth/token") {
				assert_eq!(form_body(&request)["code"], "browser-code");
				let response = if status.get() == 200 {
					json_response(
						json!({ "access_token": "access", "refresh_token": "refresh", "expires_in": 3600 }),
						200,
					)
				} else {
					json_response(
						json!({ "error": "invalid_grant", "error_description": "code expired" }),
						status.get(),
					)
				};
				return Box::pin(async { Ok(response) });
			}
			common::native_fetch(request)
		});

		let login = || async {
			let callback_page: Rc<RefCell<Option<CallbackPage>>> = Rc::default();
			let page = callback_page.clone();
			let result = radius_oauth()
				.login(
					ProviderAuthInteraction {
						notify: Rc::new(move |event| {
							let AuthEvent::AuthUrl { url, .. } = event else {
								return;
							};
							let authorize = url::Url::parse(&url).unwrap();
							let param = |name: &str| {
								authorize
									.query_pairs()
									.find(|(key, _)| key == name)
									.map(|(_, value)| value.into_owned())
									.unwrap_or_default()
							};
							let mut callback = url::Url::parse(&param("redirect_uri")).unwrap();
							callback
								.query_pairs_mut()
								.append_pair("code", "browser-code")
								.append_pair("state", &param("state"));
							let (sender, receiver) = oneshot::channel();
							pi_host::spawn_local(async move {
								let response = common::native_fetch(HttpRequest::new("GET", callback.as_str())).await;
								let page = match response {
									Ok(response) => {
										let status = response.status;
										response.text().await.map(|text| (status, text))
									}
									Err(error) => Err(error),
								};
								let _ = sender.send(page);
							});
							*page.borrow_mut() = Some(receiver);
						}),
						..interaction("browser", Rc::default())
					},
					None,
				)
				.await;
			let receiver = callback_page.borrow_mut().take().unwrap();
			let (status, page) = receiver.await.unwrap().unwrap();
			(result, status, page)
		};

		common::run(fetch, || async {
			let (failed, status, page) = login().await;
			let error = failed.unwrap_err();
			assert!(
				error.message.contains("invalid_grant: code expired"),
				"{}",
				error.message
			);
			assert_eq!(status, 502);
			assert!(page.contains("code expired"), "{page}");

			token_status.set(200);
			let (succeeded, status, page) = login().await;
			assert_eq!(succeeded.unwrap().access, "access");
			assert_eq!(status, 200);
			assert!(page.contains("Signed in to Radius."), "{page}");
		});
	}
}
