//! Port of `packages/ai/test/meta-oauth.test.ts`.
//!
//! The tests run on `pi_host::testing`'s fake timers, which advance to the next timer whenever every task waits.

mod common;

use std::cell::RefCell;
use std::collections::VecDeque;
use std::rc::Rc;

use common::{FetchHandler, form_body, json_response};
use pi_ai::auth::oauth::meta::meta_oauth;
use pi_ai::auth::types::{AuthEvent, ModelAuth, OAuthCredential, ProviderAuthInteraction};
use pi_host::{AbortController, HttpRequest, HttpResponse, PiError, PiResult};
use serde_json::{Value, json};

const CLIENT_ID: &str = "1031625952748946";
const DEVICE_AUTHORIZATION_URL: &str = "https://auth.meta.com/oidc/device/authorization/";
const DEVICE_TOKEN_URL: &str = "https://auth.meta.com/oidc/device/token/";
const MINT_URL: &str = "https://api.meta.ai/muse-code/key";
const DAY_MS: f64 = 24.0 * 60.0 * 60.0 * 1000.0;

fn handler(handle: impl Fn(HttpRequest) -> PiResult<HttpResponse> + 'static) -> FetchHandler {
	Rc::new(move |request| {
		let response = handle(request);
		Box::pin(async move { response })
	})
}

fn create_interaction(events: Rc<RefCell<Vec<Value>>>) -> ProviderAuthInteraction {
	ProviderAuthInteraction {
		signal: AbortController::new().signal(),
		prompt: Rc::new(|_| Box::pin(async { Err(PiError::new("Meta login should not prompt")) })),
		notify: Rc::new(move |event: AuthEvent| events.borrow_mut().push(serde_json::to_value(event).unwrap())),
	}
}

fn header<'a>(request: &'a HttpRequest, name: &str) -> Option<&'a str> {
	request.headers.get(name).map(String::as_str)
}

fn credential_json(credential: &OAuthCredential) -> Value {
	serde_json::to_value(credential).unwrap()
}

mod meta_oauth {
	use super::*;

	#[test]
	fn logs_in_with_the_device_flow_and_mints_a_model_api_key() {
		let poll_responses = Rc::new(RefCell::new(VecDeque::from([
			json_response(json!({ "error": "authorization_pending" }), 400),
			json_response(json!({ "access_token": "identity-token", "token_type": "Bearer" }), 200),
		])));
		let fetch = handler(move |request| {
			let url = request.url.as_str();
			if url == DEVICE_AUTHORIZATION_URL {
				assert_eq!(request.method, "POST");
				assert_eq!(form_body(&request)["client_id"], CLIENT_ID);
				return Ok(json_response(
					json!({
						"device_code": "device-code-123",
						"user_code": "ABCD-1234",
						"verification_uri": "https://auth.meta.com/oauth/device/",
						"verification_uri_complete": "https://auth.meta.com/oauth/device/?code=ABCD-1234",
						"interval": 5,
						"expires_in": 600,
					}),
					200,
				));
			}
			if url == DEVICE_TOKEN_URL {
				let params = form_body(&request);
				assert_eq!(params["grant_type"], "urn:ietf:params:oauth:grant-type:device_code");
				assert_eq!(params["client_id"], CLIENT_ID);
				assert_eq!(params["device_code"], "device-code-123");
				return poll_responses
					.borrow_mut()
					.pop_front()
					.ok_or_else(|| PiError::new("Unexpected extra token poll"));
			}
			if url == MINT_URL {
				assert_eq!(request.method, "POST");
				assert_eq!(header(&request, "Authorization"), Some("Bearer identity-token"));
				return Ok(json_response(json!({ "api_key": "LLM|minted-key" }), 200));
			}
			Err(PiError::new(format!("Unexpected fetch URL: {url}")))
		});
		common::run(fetch, || async {
			let start = pi_host::now_ms();
			let events = Rc::new(RefCell::new(Vec::new()));
			let credential = meta_oauth()
				.login(create_interaction(events.clone()), None)
				.await
				.unwrap();
			assert_eq!(
				events.borrow()[0],
				json!({
					"type": "device_code",
					"userCode": "ABCD-1234",
					"verificationUri": "https://auth.meta.com/oauth/device/?code=ABCD-1234",
					"intervalSeconds": 5.0,
					"expiresInSeconds": 600.0,
				})
			);
			assert_eq!(
				credential_json(&credential),
				json!({
					"type": "oauth",
					"refresh": "identity-token",
					"access": "LLM|minted-key",
					"expires": start + 10_000.0 + DAY_MS,
				})
			);
		});
	}

	#[test]
	fn re_mints_the_api_key_from_the_stored_identity_token_on_refresh() {
		let fetch = handler(|request| {
			assert_eq!(request.url, MINT_URL);
			assert_eq!(header(&request, "Authorization"), Some("Bearer identity-token"));
			Ok(json_response(json!({ "api_key": "LLM|fresh-key" }), 200))
		});
		common::run(fetch, || async {
			let now = pi_host::now_ms();
			let credential = meta_oauth()
				.refresh(
					OAuthCredential::new("identity-token", "LLM|old-key", 1.0),
					AbortController::new().signal(),
				)
				.await
				.unwrap();
			assert_eq!(
				credential_json(&credential),
				json!({
					"type": "oauth",
					"refresh": "identity-token",
					"access": "LLM|fresh-key",
					"expires": now + DAY_MS,
				})
			);
		});
	}

	#[test]
	fn reports_the_setup_url_when_meta_issues_no_key() {
		let fetch = handler(|_| {
			Ok(json_response(
				json!({ "require_payment": true, "action_url": "https://dev.meta.ai/billing" }),
				200,
			))
		});
		common::run(fetch, || async {
			let error = meta_oauth()
				.refresh(
					OAuthCredential::new("identity-token", "", 1.0),
					AbortController::new().signal(),
				)
				.await
				.unwrap_err();
			assert!(error.message.contains("Complete setup at https://dev.meta.ai/billing"));
		});
	}

	#[test]
	fn uses_the_minted_key_as_the_request_api_key() {
		let fetch = handler(|_| Err(PiError::new("no fetch expected")));
		common::run(fetch, || async {
			assert_eq!(
				meta_oauth()
					.to_auth(OAuthCredential::new("identity-token", "LLM|key", 1.0))
					.await
					.unwrap(),
				ModelAuth {
					api_key: Some("LLM|key".into()),
					..ModelAuth::default()
				}
			);
		});
	}
}
