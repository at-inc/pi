//! Port of `packages/ai/test/xai-oauth.test.ts`.
//!
//! The tests run on `pi_host::testing`'s fake timers, which advance to the next timer whenever every task waits,
//! so they check the recorded poll times instead of stepping the clock.

mod common;

use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::rc::Rc;

use common::{FetchHandler, form_body, json_response};
use pi_ai::auth::oauth::xai::xai_oauth;
use pi_ai::auth::types::{AuthEvent, ModelAuth, OAuthCredential, ProviderAuthInteraction};
use pi_host::{AbortController, AbortSignal, HttpRequest, HttpResponse, PiError, PiResult};
use serde_json::{Value, json};

const DEVICE_CODE_URL: &str = "https://auth.x.ai/oauth2/device/code";
const TOKEN_URL: &str = "https://auth.x.ai/oauth2/token";
const CLIENT_ID: &str = "b1a00492-073a-47ea-816f-4c329264a828";

/// `{ ...defaults, ...overrides }`; an override of `None` is `undefined` and drops the key.
fn with_overrides(mut body: Value, overrides: &[(&str, Option<Value>)]) -> Value {
	let object = body.as_object_mut().unwrap();
	for (key, value) in overrides {
		match value {
			Some(value) => {
				object.insert((*key).into(), value.clone());
			}
			None => {
				object.shift_remove(*key);
			}
		}
	}
	body
}

fn device_code_response(overrides: &[(&str, Option<Value>)]) -> Value {
	with_overrides(
		json!({
			"device_code": "device-code",
			"user_code": "ABCD-1234",
			"verification_uri": "https://accounts.x.ai/oauth2/device",
			"expires_in": 900,
			"interval": 5,
		}),
		overrides,
	)
}

fn token_response(overrides: &[(&str, Option<Value>)]) -> Value {
	with_overrides(
		json!({
			"access_token": "access-token",
			"refresh_token": "refresh-token",
			"expires_in": 21_600,
			"token_type": "Bearer",
		}),
		overrides,
	)
}

#[derive(Clone, Debug, PartialEq)]
struct DeviceCodeInfo {
	user_code: String,
	verification_uri: String,
	interval_seconds: Option<f64>,
	expires_in_seconds: Option<f64>,
}

fn handler(handle: impl Fn(HttpRequest) -> PiResult<HttpResponse> + 'static) -> FetchHandler {
	Rc::new(move |request| {
		let response = handle(request);
		Box::pin(async move { response })
	})
}

async fn login_xai_for_test(
	on_device_code: impl Fn(DeviceCodeInfo) + 'static,
	signal: Option<AbortSignal>,
) -> PiResult<OAuthCredential> {
	xai_oauth()
		.login(
			ProviderAuthInteraction {
				signal: signal.unwrap_or_else(|| AbortController::new().signal()),
				prompt: Rc::new(|_| Box::pin(async { Err(PiError::new("Unexpected prompt")) })),
				notify: Rc::new(move |event| {
					if let AuthEvent::DeviceCode {
						user_code,
						verification_uri,
						interval_seconds,
						expires_in_seconds,
						..
					} = event
					{
						on_device_code(DeviceCodeInfo {
							user_code,
							verification_uri,
							interval_seconds,
							expires_in_seconds,
						});
					}
				}),
			},
			None,
		)
		.await
}

async fn refresh_xai_for_test(refresh_token: &str) -> PiResult<OAuthCredential> {
	xai_oauth()
		.refresh(
			OAuthCredential::new(refresh_token, "old-access", 0.0),
			AbortController::new().signal(),
		)
		.await
}

fn url_of(request: &HttpRequest) -> &str {
	request.url.as_str()
}

mod x_ai_oauth_device_flow {
	use super::*;

	#[test]
	fn uses_the_device_grant_delays_polling_and_handles_pending_and_slow_down() {
		let poll_times: Rc<RefCell<Vec<f64>>> = Rc::new(RefCell::new(Vec::new()));
		let token_replies = Rc::new(RefCell::new(VecDeque::from([
			json_response(json!({ "error": "authorization_pending" }), 400),
			json_response(json!({ "error": "slow_down", "interval": 10 }), 400),
			json_response(token_response(&[]), 200),
		])));
		let recorded = poll_times.clone();
		let fetch = handler(move |request| {
			let form = form_body(&request);
			if url_of(&request) == DEVICE_CODE_URL {
				assert_eq!(form["client_id"], CLIENT_ID);
				assert_eq!(
					form["scope"],
					"openid profile email offline_access grok-cli:access api:access"
				);
				assert_eq!(form["referrer"], "pi");
				return Ok(json_response(device_code_response(&[]), 200));
			}
			if url_of(&request) == TOKEN_URL {
				recorded.borrow_mut().push(pi_host::now_ms());
				assert_eq!(form["grant_type"], "urn:ietf:params:oauth:grant-type:device_code");
				assert_eq!(form["client_id"], CLIENT_ID);
				assert_eq!(form["device_code"], "device-code");
				return token_replies
					.borrow_mut()
					.pop_front()
					.ok_or_else(|| PiError::new("Unexpected token poll"));
			}
			Err(PiError::new(format!("Unexpected request: {}", request.url)))
		});
		common::run(fetch, || async {
			let start = pi_host::now_ms();
			let device_codes: Rc<RefCell<Vec<DeviceCodeInfo>>> = Rc::new(RefCell::new(Vec::new()));
			let polls_at_notify = Rc::new(Cell::new(usize::MAX));
			let codes = device_codes.clone();
			let at_notify = polls_at_notify.clone();
			let polls = poll_times.clone();
			let credentials = login_xai_for_test(
				move |info| {
					codes.borrow_mut().push(info);
					at_notify.set(polls.borrow().len());
				},
				None,
			)
			.await
			.unwrap();
			assert_eq!(
				*device_codes.borrow(),
				vec![DeviceCodeInfo {
					user_code: "ABCD-1234".into(),
					verification_uri: "https://accounts.x.ai/oauth2/device".into(),
					interval_seconds: Some(5.0),
					expires_in_seconds: Some(900.0),
				}]
			);
			assert_eq!(polls_at_notify.get(), 0);
			// slow_down raised the interval to 10 seconds
			assert_eq!(
				*poll_times.borrow(),
				vec![start + 5000.0, start + 10_000.0, start + 20_000.0]
			);
			assert_eq!(
				serde_json::to_value(&credentials).unwrap(),
				json!({
					"type": "oauth",
					"refresh": "refresh-token",
					"access": "access-token",
					"expires": start + 20_000.0 + 21_600_000.0 - 300_000.0,
				})
			);
		});
	}

	#[test]
	fn falls_back_to_the_default_poll_interval_when_the_response_reports_interval_0() {
		let poll_times: Rc<RefCell<Vec<f64>>> = Rc::new(RefCell::new(Vec::new()));
		let recorded = poll_times.clone();
		let fetch = handler(move |request| {
			if url_of(&request) == DEVICE_CODE_URL {
				return Ok(json_response(
					device_code_response(&[("interval", Some(json!(0)))]),
					200,
				));
			}
			recorded.borrow_mut().push(pi_host::now_ms());
			Ok(json_response(token_response(&[]), 200))
		});
		common::run(fetch, || async {
			let start = pi_host::now_ms();
			login_xai_for_test(|_| {}, None).await.unwrap();
			// RFC 8628 default interval is 5 seconds when the server does not require a wait.
			assert_eq!(*poll_times.borrow(), vec![start + 5000.0]);
		});
	}

	#[test]
	fn prefers_verification_uri_complete_when_the_server_provides_it() {
		let fetch = handler(|request| {
			if url_of(&request) == DEVICE_CODE_URL {
				return Ok(json_response(
					device_code_response(&[(
						"verification_uri_complete",
						Some(json!("https://accounts.x.ai/oauth2/device?user_code=ABCD-1234")),
					)]),
					200,
				));
			}
			Ok(json_response(token_response(&[]), 200))
		});
		common::run(fetch, || async {
			let device_codes: Rc<RefCell<Vec<DeviceCodeInfo>>> = Rc::new(RefCell::new(Vec::new()));
			let codes = device_codes.clone();
			login_xai_for_test(move |info| codes.borrow_mut().push(info), None)
				.await
				.unwrap();
			assert_eq!(
				*device_codes.borrow(),
				vec![DeviceCodeInfo {
					user_code: "ABCD-1234".into(),
					verification_uri: "https://accounts.x.ai/oauth2/device?user_code=ABCD-1234".into(),
					interval_seconds: Some(5.0),
					expires_in_seconds: Some(900.0),
				}]
			);
		});
	}

	#[test]
	fn rejects_a_non_https_verification_uri_complete() {
		let fetch = handler(|_| {
			Ok(json_response(
				device_code_response(&[(
					"verification_uri_complete",
					Some(json!("http://accounts.x.ai/oauth2/device?user_code=ABCD-1234")),
				)]),
				200,
			))
		});
		common::run(fetch, || async {
			let error = login_xai_for_test(|_| {}, None).await.unwrap_err();
			assert!(error.message.contains("Untrusted verification URI"));
		});
	}

	#[test]
	fn rejects_a_non_https_verification_uri() {
		for verification_uri in ["http://accounts.x.ai/oauth2/device", "file:///etc/passwd", "not a url"] {
			let fetch = handler(move |_| {
				Ok(json_response(
					device_code_response(&[("verification_uri", Some(json!(verification_uri)))]),
					200,
				))
			});
			common::run(fetch, || async {
				let error = login_xai_for_test(|_| {}, None).await.unwrap_err();
				assert!(
					error.message.contains("Untrusted verification URI"),
					"{verification_uri}"
				);
			});
		}
	}

	#[test]
	fn fails_when_device_authorization_is_denied() {
		for error in ["access_denied", "authorization_denied"] {
			let request_count = Rc::new(Cell::new(0));
			let fetch = handler(move |_| {
				request_count.set(request_count.get() + 1);
				Ok(if request_count.get() == 1 {
					json_response(device_code_response(&[("interval", Some(json!(1)))]), 200)
				} else {
					json_response(json!({ "error": error }), 400)
				})
			});
			common::run(fetch, || async {
				let failure = login_xai_for_test(|_| {}, None).await.unwrap_err();
				assert!(
					failure.message.contains("xAI device authorization was denied"),
					"{error}"
				);
			});
		}
	}

	#[test]
	fn cancels_while_waiting_for_the_first_token_poll() {
		let calls = Rc::new(Cell::new(0));
		let counted = calls.clone();
		let fetch = handler(move |_| {
			counted.set(counted.get() + 1);
			Ok(json_response(device_code_response(&[]), 200))
		});
		common::run(fetch, || async {
			let controller = AbortController::new();
			let aborter = controller.clone();
			let error = login_xai_for_test(move |_| aborter.abort(), Some(controller.signal()))
				.await
				.unwrap_err();
			assert!(error.message.contains("Login cancelled"));
			assert_eq!(calls.get(), 1);
		});
	}

	#[test]
	fn refreshes_tokens_and_preserves_an_unrotated_refresh_token() {
		let request_count = Rc::new(Cell::new(0));
		let fetch = handler(move |request| {
			assert_eq!(url_of(&request), TOKEN_URL);
			let form = form_body(&request);
			assert_eq!(form["grant_type"], "refresh_token");
			assert_eq!(form["client_id"], CLIENT_ID);
			request_count.set(request_count.get() + 1);
			if request_count.get() == 1 {
				assert_eq!(form["refresh_token"], "old-refresh");
				return Ok(json_response(
					token_response(&[
						("access_token", Some(json!("new-access"))),
						("refresh_token", Some(json!("new-refresh"))),
					]),
					200,
				));
			}
			assert_eq!(form["refresh_token"], "keep-refresh");
			Ok(json_response(
				token_response(&[("access_token", Some(json!("newer-access"))), ("refresh_token", None)]),
				200,
			))
		});
		common::run(fetch, || async {
			let rotated = refresh_xai_for_test("old-refresh").await.unwrap();
			let preserved = refresh_xai_for_test("keep-refresh").await.unwrap();
			assert_eq!(serde_json::to_value(&rotated).unwrap()["type"], "oauth");
			assert_eq!(rotated.refresh, "new-refresh");
			assert_eq!(rotated.access, "new-access");
			assert_eq!(preserved.refresh, "keep-refresh");
			assert_eq!(preserved.access, "newer-access");
			let oauth = xai_oauth();
			assert_eq!(oauth.name, "xAI (Grok/X subscription)");
			assert_eq!(
				oauth.to_auth(preserved).await.unwrap(),
				ModelAuth {
					api_key: Some("newer-access".into()),
					..ModelAuth::default()
				}
			);
		});
	}

	#[test]
	fn assumes_a_one_hour_lifetime_when_expires_in_is_missing() {
		let fetch = handler(|_| Ok(json_response(token_response(&[("expires_in", None)]), 200)));
		common::run(fetch, || async {
			let start = pi_host::now_ms();
			let credentials = refresh_xai_for_test("old-refresh").await.unwrap();
			assert_eq!(credentials.expires, start + 3_600_000.0 - 300_000.0);
		});
	}

	#[test]
	fn rejects_token_responses_with_missing_fields() {
		let fetch = handler(|_| Ok(json_response(token_response(&[("access_token", None)]), 200)));
		common::run(fetch, || async {
			let error = refresh_xai_for_test("old-refresh").await.unwrap_err();
			assert!(error.message.contains("Invalid xAI OAuth response field: access_token"));
		});
	}

	#[test]
	fn surfaces_the_upstream_error_code_and_description_on_refresh_failure() {
		let fetch = handler(|_| {
			Ok(json_response(
				json!({ "error": "invalid_grant", "error_description": "refresh token revoked" }),
				400,
			))
		});
		common::run(fetch, || async {
			let error = refresh_xai_for_test("old-refresh").await.unwrap_err();
			assert!(
				error
					.message
					.contains("xAI OAuth token refresh failed (HTTP 400): invalid_grant: refresh token revoked")
			);
		});
	}
}
