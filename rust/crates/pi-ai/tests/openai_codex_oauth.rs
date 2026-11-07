//! Port of `packages/ai/test/openai-codex-oauth.test.ts`.
//!
//! The device-code tests run on `pi_host::testing`'s fake timers and check the recorded poll times instead of
//! stepping the clock. The browser-login tests try the fixed callback port 1455 and hold `common::sequential()`.

mod common;

use std::cell::RefCell;
use std::collections::VecDeque;
use std::rc::Rc;

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use common::{form_body, json_body, json_response};
use indexmap::IndexMap;
use pi_ai::auth::oauth::openai_codex::openai_codex_oauth;
use pi_ai::auth::types::{
	AuthEvent, AuthPromptKind, AuthSelectOption, LoginOptions, OAuthCredential, ProviderAuthInteraction,
};
use pi_host::{AbortController, AbortSignal, HttpRequest, HttpResponse, PiError, PiResult};
use serde_json::{Value, json};
use url::Url;

const START: f64 = 1_700_000_000_000.0;
const USER_CODE_URL: &str = "https://auth.openai.com/api/accounts/deviceauth/usercode";
const DEVICE_TOKEN_URL: &str = "https://auth.openai.com/api/accounts/deviceauth/token";
const TOKEN_URL: &str = "https://auth.openai.com/oauth/token";

fn create_access_token(account_id: &str) -> String {
	let header = STANDARD.encode(pi_host::js::json_stringify(&json!({ "alg": "none" })));
	let payload = STANDARD.encode(pi_host::js::json_stringify(&json!({
		"https://api.openai.com/auth": { "chatgpt_account_id": account_id },
	})));
	format!("{header}.{payload}.signature")
}

fn device_auth_pending_response() -> HttpResponse {
	json_response(
		json!({
			"error": {
				"message": "Device authorization is pending. Please try again.",
				"type": "invalid_request_error",
				"param": null,
				"code": "deviceauth_authorization_pending",
			},
		}),
		403,
	)
}

#[derive(Clone, Debug, PartialEq)]
struct DeviceInfo {
	user_code: String,
	verification_uri: String,
	interval_seconds: Option<f64>,
	expires_in_seconds: Option<f64>,
}

fn device_code_interaction(
	device_infos: Rc<RefCell<Vec<DeviceInfo>>>,
	signal: Option<AbortSignal>,
) -> ProviderAuthInteraction {
	ProviderAuthInteraction {
		signal: signal.unwrap_or_else(|| AbortController::new().signal()),
		prompt: Rc::new(|prompt| {
			Box::pin(async move {
				if !matches!(prompt.kind, AuthPromptKind::Select { .. }) {
					return Err(PiError::new(format!("Unexpected prompt: {}", prompt.type_name())));
				}
				Ok("device_code".to_string())
			})
		}),
		notify: Rc::new(move |event| {
			if let AuthEvent::DeviceCode {
				user_code,
				verification_uri,
				interval_seconds,
				expires_in_seconds,
				..
			} = event
			{
				device_infos.borrow_mut().push(DeviceInfo {
					user_code,
					verification_uri,
					interval_seconds,
					expires_in_seconds,
				});
			}
		}),
	}
}

type Router = Rc<dyn Fn(&HttpRequest) -> PiResult<HttpResponse>>;

fn serve(host: &pi_host::testing::TestHost, router: Router) {
	host.on_fetch(move |request| {
		let response = router(&request);
		Box::pin(async move { response })
	});
}

fn shift(responses: &Rc<RefCell<VecDeque<HttpResponse>>>) -> PiResult<HttpResponse> {
	responses
		.borrow_mut()
		.pop_front()
		.ok_or_else(|| PiError::new("Unexpected extra device auth poll"))
}

fn content_type(request: &HttpRequest) -> Option<&str> {
	request.header("Content-Type")
}

fn login_device_code(
	device_infos: Rc<RefCell<Vec<DeviceInfo>>>,
	signal: Option<AbortSignal>,
) -> pi_host::LocalBoxFuture<'static, PiResult<OAuthCredential>> {
	openai_codex_oauth().login(device_code_interaction(device_infos, signal), None)
}

mod openai_codex_oauth_suite {
	use super::*;

	#[test]
	fn logs_in_with_the_openai_codex_device_code_flow() {
		let access_token = create_access_token("account-123");
		let device_infos = Rc::new(RefCell::new(Vec::new()));
		let poll_times = Rc::new(RefCell::new(Vec::new()));
		let poll_responses = Rc::new(RefCell::new(VecDeque::from([
			device_auth_pending_response(),
			json_response(
				json!({
					"authorization_code": "oauth-code",
					"code_challenge": "device-code-challenge",
					"code_verifier": "device-code-verifier",
				}),
				200,
			),
		])));
		let credential = pi_host::testing::run(|host| {
			let access_token = access_token.clone();
			let poll_times = poll_times.clone();
			serve(
				&host,
				Rc::new(move |request| {
					if request.url == USER_CODE_URL {
						assert_eq!(request.method, "POST");
						assert_eq!(content_type(request), Some("application/json"));
						assert_eq!(
							json_body(request),
							json!({ "client_id": "app_EMoamEEZ73f0CkXaXp7hrann" })
						);
						return Ok(json_response(
							json!({ "device_auth_id": "device-auth-id", "user_code": "ABCD-1234", "interval": "5" }),
							200,
						));
					}
					if request.url == DEVICE_TOKEN_URL {
						poll_times.borrow_mut().push(pi_host::now_ms());
						assert_eq!(request.method, "POST");
						assert_eq!(content_type(request), Some("application/json"));
						assert_eq!(
							json_body(request),
							json!({ "device_auth_id": "device-auth-id", "user_code": "ABCD-1234" })
						);
						return shift(&poll_responses);
					}
					if request.url == TOKEN_URL {
						assert_eq!(request.method, "POST");
						assert_eq!(content_type(request), Some("application/x-www-form-urlencoded"));
						let params = form_body(request);
						assert_eq!(params["grant_type"], "authorization_code");
						assert_eq!(params["client_id"], "app_EMoamEEZ73f0CkXaXp7hrann");
						assert_eq!(params["code"], "oauth-code");
						assert_eq!(params["redirect_uri"], "https://auth.openai.com/deviceauth/callback");
						assert_eq!(params["code_verifier"], "device-code-verifier");
						return Ok(json_response(
							json!({ "access_token": access_token, "refresh_token": "refresh-token", "expires_in": 3600 }),
							200,
						));
					}
					Err(PiError::new(format!("Unexpected fetch URL: {}", request.url)))
				}),
			);
			login_device_code(device_infos.clone(), None)
		})
		.unwrap();

		assert_eq!(
			*device_infos.borrow(),
			vec![DeviceInfo {
				user_code: "ABCD-1234".into(),
				verification_uri: "https://auth.openai.com/codex/device".into(),
				interval_seconds: Some(5.0),
				expires_in_seconds: Some(900.0),
			}]
		);
		assert_eq!(credential.access, access_token);
		assert_eq!(credential.refresh, "refresh-token");
		assert_eq!(credential.expires, START + 5000.0 + 3600.0 * 1000.0);
		assert_eq!(credential.field("accountId"), Some(&json!("account-123")));
		assert_eq!(*poll_times.borrow(), vec![START, START + 5000.0]);
	}

	#[test]
	fn offers_browser_login_first_and_uses_the_selected_openai_codex_device_code_flow() {
		let access_token = create_access_token("account-456");
		let select_prompts = Rc::new(RefCell::new(Vec::new()));
		let device_infos = Rc::new(RefCell::new(Vec::new()));
		let credential = pi_host::testing::run(|host| {
			let access_token = access_token.clone();
			serve(
				&host,
				Rc::new(move |request| {
					if request.url == USER_CODE_URL {
						assert_eq!(
							json_body(request),
							json!({ "client_id": "app_EMoamEEZ73f0CkXaXp7hrann" })
						);
						return Ok(json_response(
							json!({ "device_auth_id": "device-auth-id", "user_code": "WXYZ-7890", "interval": "5" }),
							200,
						));
					}
					if request.url == DEVICE_TOKEN_URL {
						return Ok(json_response(
							json!({
								"authorization_code": "oauth-code",
								"code_challenge": "device-code-challenge",
								"code_verifier": "device-code-verifier",
							}),
							200,
						));
					}
					if request.url == TOKEN_URL {
						return Ok(json_response(
							json!({ "access_token": access_token, "refresh_token": "refresh-token", "expires_in": 3600 }),
							200,
						));
					}
					Err(PiError::new(format!("Unexpected fetch URL: {}", request.url)))
				}),
			);
			let selects = select_prompts.clone();
			let infos = device_infos.clone();
			openai_codex_oauth().login(
				ProviderAuthInteraction {
					signal: AbortController::new().signal(),
					prompt: Rc::new(move |prompt| {
						let selects = selects.clone();
						Box::pin(async move {
							if !matches!(prompt.kind, AuthPromptKind::Select { .. }) {
								return Err(PiError::new("Text prompt should not be used"));
							}
							selects.borrow_mut().push(prompt.kind.clone());
							Ok("device_code".to_string())
						})
					}),
					notify: Rc::new(move |event| match event {
						AuthEvent::AuthUrl { .. } => panic!("Browser login should not start"),
						AuthEvent::DeviceCode {
							user_code,
							verification_uri,
							interval_seconds,
							expires_in_seconds,
							..
						} => infos.borrow_mut().push(DeviceInfo {
							user_code,
							verification_uri,
							interval_seconds,
							expires_in_seconds,
						}),
						_ => {}
					}),
				},
				None,
			)
		})
		.unwrap();

		assert_eq!(
			serde_json::to_value(&credential).unwrap()["type"],
			Value::String("oauth".into())
		);
		assert_eq!(credential.access, access_token);
		assert_eq!(credential.refresh, "refresh-token");
		assert_eq!(credential.field("accountId"), Some(&json!("account-456")));
		assert_eq!(
			*select_prompts.borrow(),
			vec![AuthPromptKind::Select {
				message: "Select OpenAI Codex login method:".into(),
				options: vec![
					AuthSelectOption::new("browser", "Browser login (default)"),
					AuthSelectOption::new("device_code", "Device code login (headless)"),
				],
			}]
		);
		assert_eq!(
			*device_infos.borrow(),
			vec![DeviceInfo {
				user_code: "WXYZ-7890".into(),
				verification_uri: "https://auth.openai.com/codex/device".into(),
				interval_seconds: Some(5.0),
				expires_in_seconds: Some(900.0),
			}]
		);
	}

	#[test]
	fn cancels_when_openai_codex_login_method_selection_is_cancelled() {
		let error = pi_host::testing::run(|_| {
			openai_codex_oauth().login(
				ProviderAuthInteraction {
					signal: AbortController::new().signal(),
					prompt: Rc::new(|_| Box::pin(async { Err(PiError::new("Login cancelled")) })),
					notify: Rc::new(|_| {}),
				},
				None,
			)
		})
		.unwrap_err();
		assert!(error.message.contains("Login cancelled"));
	}

	#[test]
	fn cancels_the_openai_codex_device_code_flow_while_waiting() {
		let controller = AbortController::new();
		let poll_times = Rc::new(RefCell::new(Vec::new()));
		let error = pi_host::testing::run(|host| {
			let poll_times = poll_times.clone();
			let signal = controller.signal();
			let controller = controller.clone();
			serve(
				&host,
				Rc::new(move |request| {
					if request.url == USER_CODE_URL {
						assert_eq!(
							json_body(request),
							json!({ "client_id": "app_EMoamEEZ73f0CkXaXp7hrann" })
						);
						return Ok(json_response(
							json!({ "device_auth_id": "device-auth-id", "user_code": "ABCD-1234", "interval": "5" }),
							200,
						));
					}
					if request.url == DEVICE_TOKEN_URL {
						poll_times.borrow_mut().push(pi_host::now_ms());
						// The test aborts once the first poll has happened.
						controller.abort();
						return Ok(device_auth_pending_response());
					}
					Err(PiError::new(format!("Unexpected fetch URL: {}", request.url)))
				}),
			);
			login_device_code(Rc::new(RefCell::new(Vec::new())), Some(signal))
		})
		.unwrap_err();
		assert_eq!(poll_times.borrow().len(), 1);
		assert_eq!(error.message, "Login cancelled");
	}

	#[test]
	fn times_out_the_openai_codex_device_code_flow_after_15_minutes() {
		let poll_times = Rc::new(RefCell::new(Vec::new()));
		let error = pi_host::testing::run(|host| {
			let poll_times = poll_times.clone();
			serve(
				&host,
				Rc::new(move |request| {
					if request.url == USER_CODE_URL {
						assert_eq!(
							json_body(request),
							json!({ "client_id": "app_EMoamEEZ73f0CkXaXp7hrann" })
						);
						return Ok(json_response(
							json!({ "device_auth_id": "device-auth-id", "user_code": "ABCD-1234", "interval": "60" }),
							200,
						));
					}
					if request.url == DEVICE_TOKEN_URL {
						poll_times.borrow_mut().push(pi_host::now_ms());
						return Ok(device_auth_pending_response());
					}
					Err(PiError::new(format!("Unexpected fetch URL: {}", request.url)))
				}),
			);
			login_device_code(Rc::new(RefCell::new(Vec::new())), None)
		})
		.unwrap_err();
		assert_eq!(poll_times.borrow().first(), Some(&START));
		assert_eq!(error.message, "Device flow timed out");
	}

	#[test]
	fn treats_openai_codex_device_auth_403_and_404_responses_as_pending() {
		let access_token = create_access_token("account-403-404");
		let poll_times = Rc::new(RefCell::new(Vec::new()));
		let mut text_headers = IndexMap::new();
		text_headers.insert("Content-Type".to_string(), "text/plain".to_string());
		let poll_responses = Rc::new(RefCell::new(VecDeque::from([
			json_response(json!({ "error": "access_denied", "error_description": "denied" }), 403),
			HttpResponse::from_bytes(404, text_headers, "not ready"),
			json_response(
				json!({
					"authorization_code": "oauth-code",
					"code_challenge": "device-code-challenge",
					"code_verifier": "device-code-verifier",
				}),
				200,
			),
		])));
		let credential = pi_host::testing::run(|host| {
			let access_token = access_token.clone();
			let poll_times = poll_times.clone();
			serve(
				&host,
				Rc::new(move |request| {
					if request.url == USER_CODE_URL {
						return Ok(json_response(
							json!({ "device_auth_id": "device-auth-id", "user_code": "ABCD-1234", "interval": "1" }),
							200,
						));
					}
					if request.url == DEVICE_TOKEN_URL {
						poll_times.borrow_mut().push(pi_host::now_ms());
						return shift(&poll_responses);
					}
					if request.url == TOKEN_URL {
						return Ok(json_response(
							json!({ "access_token": access_token, "refresh_token": "refresh-token", "expires_in": 3600 }),
							200,
						));
					}
					Err(PiError::new(format!("Unexpected fetch URL: {}", request.url)))
				}),
			);
			login_device_code(Rc::new(RefCell::new(Vec::new())), None)
		})
		.unwrap();

		assert_eq!(credential.access, access_token);
		assert_eq!(credential.refresh, "refresh-token");
		assert_eq!(credential.field("accountId"), Some(&json!("account-403-404")));
		assert_eq!(*poll_times.borrow(), vec![START, START + 1000.0, START + 2000.0]);
	}

	#[test]
	fn includes_the_response_body_in_openai_codex_device_auth_poll_failures() {
		let error = pi_host::testing::run(|host| {
			serve(
				&host,
				Rc::new(|request| {
					if request.url == USER_CODE_URL {
						return Ok(json_response(
							json!({ "device_auth_id": "device-auth-id", "user_code": "ABCD-1234", "interval": "5" }),
							200,
						));
					}
					if request.url == DEVICE_TOKEN_URL {
						return Ok(json_response(
							json!({ "error": "server_error", "error_description": "try again later" }),
							500,
						));
					}
					Err(PiError::new(format!("Unexpected fetch URL: {}", request.url)))
				}),
			);
			login_device_code(Rc::new(RefCell::new(Vec::new())), None)
		})
		.unwrap_err();
		assert!(error.message.contains(
			r#"OpenAI Codex device auth failed with status 500: {"error":"server_error","error_description":"try again later"}"#
		));
	}

	#[test]
	fn does_not_write_token_refresh_failures_to_stderr() {
		let error = pi_host::testing::run(|host| {
			serve(
				&host,
				Rc::new(|_| {
					let mut response = json_response(
						json!({
							"error": {
								"message": "Could not validate your token. Please try signing in again.",
								"type": "invalid_request_error",
							},
						}),
						401,
					);
					response.status_text = "Unauthorized".into();
					Ok(response)
				}),
			);
			openai_codex_oauth().refresh(
				OAuthCredential::new("invalid-refresh-token", "invalid-access-token", 0.0),
				AbortController::new().signal(),
			)
		})
		.unwrap_err();
		let pattern =
			regex::Regex::new(r"OpenAI Codex token refresh failed \(401\).*Could not validate your token").unwrap();
		assert!(pattern.is_match(&error.message), "{}", error.message);
	}

	fn pasted_redirect_interaction(auth_url: Rc<RefCell<String>>) -> ProviderAuthInteraction {
		let notified = auth_url.clone();
		ProviderAuthInteraction {
			signal: AbortController::new().signal(),
			notify: Rc::new(move |event| {
				if let AuthEvent::AuthUrl { url, .. } = event {
					*notified.borrow_mut() = url;
				}
			}),
			prompt: Rc::new(move |prompt| {
				let auth_url = auth_url.borrow().clone();
				Box::pin(async move {
					match prompt.kind {
						AuthPromptKind::Select { .. } => Ok("browser".to_string()),
						AuthPromptKind::ManualCode { .. } => {
							let state = Url::parse(&auth_url)
								.unwrap()
								.query_pairs()
								.find(|(key, _)| key == "state")
								.map(|(_, value)| value.into_owned())
								.unwrap_or_default();
							Ok(format!(
								"http://localhost:1455/auth/callback?code=pasted-code&state={state}"
							))
						}
						_ => Err(PiError::new(format!("Unexpected prompt: {}", prompt.type_name()))),
					}
				})
			}),
		}
	}

	#[test]
	fn uses_the_app_s_agent_name_as_the_browser_login_originator() {
		let _sequential = common::sequential();
		let auth_url = Rc::new(RefCell::new(String::new()));
		common::run(
			Rc::new(|_| {
				Box::pin(async {
					Ok(json_response(
						json!({ "access_token": create_access_token("acct"), "refresh_token": "refresh", "expires_in": 3600 }),
						200,
					))
				})
			}),
			|| {
				openai_codex_oauth().login(
					pasted_redirect_interaction(auth_url.clone()),
					Some(LoginOptions {
						agent_name: Some("my-app".into()),
						..LoginOptions::default()
					}),
				)
			},
		)
		.unwrap();

		let originator = Url::parse(&auth_url.borrow())
			.unwrap()
			.query_pairs()
			.find(|(key, _)| key == "originator")
			.map(|(_, value)| value.into_owned());
		assert_eq!(originator.as_deref(), Some("my-app"));
	}

	#[test]
	fn falls_back_to_the_pasted_redirect_url_when_the_fixed_callback_port_is_taken() {
		let _sequential = common::sequential();
		// Port 1455 is registered with OpenAI; the Codex CLI may hold it. Occupy it unless it already is.
		let blocker = std::net::TcpListener::bind("127.0.0.1:1455").ok();
		let exchange_body: Rc<RefCell<Option<IndexMap<String, String>>>> = Rc::new(RefCell::new(None));
		let recorded = exchange_body.clone();
		let auth_url = Rc::new(RefCell::new(String::new()));
		let credential = common::run(
			Rc::new(move |request| {
				assert_eq!(request.url, TOKEN_URL);
				*recorded.borrow_mut() = Some(form_body(&request));
				Box::pin(async {
					Ok(json_response(
						json!({ "access_token": create_access_token("acct"), "refresh_token": "refresh", "expires_in": 3600 }),
						200,
					))
				})
			}),
			|| openai_codex_oauth().login(pasted_redirect_interaction(auth_url.clone()), None),
		)
		.unwrap();
		drop(blocker);

		assert_eq!(credential.field("accountId"), Some(&json!("acct")));
		let exchange_body = exchange_body.borrow();
		let exchange_body = exchange_body.as_ref().unwrap();
		assert_eq!(exchange_body["code"], "pasted-code");
		assert_eq!(exchange_body["redirect_uri"], "http://localhost:1455/auth/callback");
	}
}
