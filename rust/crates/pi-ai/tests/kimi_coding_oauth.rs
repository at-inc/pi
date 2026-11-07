//! Port of `packages/ai/test/kimi-coding-oauth.test.ts`.
//!
//! The tests run on `pi_host::testing`'s fake timers, which advance to the next timer whenever every task waits, so
//! they check recorded poll times instead of stepping the clock.

// The options' fields sit behind `Deref`, so the struct literal this lint suggests does not compile.
#![allow(clippy::field_reassign_with_default)]

mod common;

use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::rc::Rc;

use common::{FetchHandler, form_body, json_response, run_with_env_and_fake_timers};
use indexmap::IndexMap;
use pi_ai::auth::credential_store::InMemoryCredentialStore;
use pi_ai::auth::oauth::kimi_coding::kimi_coding_oauth;
use pi_ai::auth::types::{
	AuthEvent, AuthPrompt, AuthPromptKind, Credential, CredentialStore, ModelAuth, OAuthCredential,
	ProviderAuthInteraction,
};
use pi_ai::models::{CreateModelsOptions, ModelsSimpleStreamOptions, create_models};
use pi_ai::providers::kimi_coding::kimi_coding_provider;
use pi_ai::types::{Context, Message, StopReason, UserMessage, UserMessageContent};
use pi_host::{AbortController, HttpRequest, HttpResponse, LocalBoxFuture, PiError, PiResult};
use serde_json::{Value, json};

const CLIENT_ID: &str = "17e5f671-d194-4dfb-9706-5516cb48c098";
const OAUTH_HOST: &str = "https://auth.kimi.com";

/// `beforeEach`: the Kimi endpoint overrides are stubbed to `""`.
const EMPTY_ENV: [(&str, &str); 3] = [
	("KIMI_CODE_OAUTH_HOST", ""),
	("KIMI_OAUTH_HOST", ""),
	("KIMI_CODE_BASE_URL", ""),
];

fn env_with<'a>(name: &'a str, value: &'a str) -> Vec<(&'a str, &'a str)> {
	let mut env: Vec<(&str, &str)> = EMPTY_ENV.iter().filter(|(key, _)| *key != name).copied().collect();
	env.push((name, value));
	env
}

fn handler(handle: impl Fn(HttpRequest) -> PiResult<HttpResponse> + 'static) -> FetchHandler {
	Rc::new(move |request| {
		let response = handle(request);
		Box::pin(async move { response })
	})
}

fn device_authorization_response(overrides: Value) -> HttpResponse {
	let mut body = json!({
		"user_code": "ABCD-1234",
		"device_code": "device-code-123",
		"verification_uri": "https://www.kimi.com/code",
		"verification_uri_complete": "https://www.kimi.com/code?user_code=ABCD-1234",
		"interval": 5,
		"expires_in": 600,
	});
	for (key, value) in overrides.as_object().unwrap() {
		body[key] = value.clone();
	}
	json_response(body, 200)
}

fn create_interaction(events: Rc<RefCell<Vec<Value>>>, region: &'static str) -> ProviderAuthInteraction {
	with_prompt(events, move |prompt| {
		assert!(matches!(prompt.kind, AuthPromptKind::Select { .. }));
		Box::pin(async move { Ok(region.to_string()) })
	})
}

fn with_prompt(
	events: Rc<RefCell<Vec<Value>>>,
	prompt: impl Fn(AuthPrompt) -> LocalBoxFuture<'static, PiResult<String>> + 'static,
) -> ProviderAuthInteraction {
	ProviderAuthInteraction {
		signal: AbortController::new().signal(),
		prompt: Rc::new(prompt),
		notify: Rc::new(move |event: AuthEvent| events.borrow_mut().push(serde_json::to_value(event).unwrap())),
	}
}

fn no_events() -> Rc<RefCell<Vec<Value>>> {
	Rc::new(RefCell::new(Vec::new()))
}

fn credential_json(credential: &OAuthCredential) -> Value {
	serde_json::to_value(credential).unwrap()
}

/// `expect(actual).toMatchObject(expected)` for flat objects.
fn assert_matches(actual: &Value, expected: Value) {
	for (key, value) in expected.as_object().unwrap() {
		assert_eq!(&actual[key], value, "{key}");
	}
}

mod kimi_code_oauth {
	use super::*;

	#[test]
	fn logs_in_with_the_device_authorization_flow() {
		let poll_responses = Rc::new(RefCell::new(VecDeque::from([
			json_response(json!({ "error": "authorization_pending" }), 400),
			json_response(
				json!({ "access_token": "access-token", "refresh_token": "refresh-token", "expires_in": 3600 }),
				200,
			),
		])));
		let poll_times: Rc<RefCell<Vec<f64>>> = Rc::new(RefCell::new(Vec::new()));
		let recorded = poll_times.clone();
		let fetch = handler(move |request| {
			let url = request.url.as_str();
			if url == format!("{OAUTH_HOST}/api/oauth/device_authorization") {
				assert_eq!(request.method, "POST");
				assert_eq!(
					request.headers.get("Content-Type").map(String::as_str),
					Some("application/x-www-form-urlencoded")
				);
				assert_eq!(
					request.headers.get("Accept").map(String::as_str),
					Some("application/json")
				);
				assert_eq!(form_body(&request)["client_id"], CLIENT_ID);
				return Ok(device_authorization_response(json!({})));
			}
			if url == format!("{OAUTH_HOST}/api/oauth/token") {
				recorded.borrow_mut().push(pi_host::now_ms());
				let params = form_body(&request);
				assert_eq!(params["grant_type"], "urn:ietf:params:oauth:grant-type:device_code");
				assert_eq!(params["client_id"], CLIENT_ID);
				assert_eq!(params["device_code"], "device-code-123");
				return poll_responses
					.borrow_mut()
					.pop_front()
					.ok_or_else(|| PiError::new("Unexpected extra token poll"));
			}
			Err(PiError::new(format!("Unexpected fetch URL: {url}")))
		});
		run_with_env_and_fake_timers(fetch, &EMPTY_ENV, || async {
			let start = pi_host::now_ms();
			let events = no_events();
			let credential = kimi_coding_oauth()
				.login(create_interaction(events.clone(), "mainland-cn"), None)
				.await
				.unwrap();
			assert_eq!(
				*events.borrow(),
				vec![json!({
					"type": "device_code",
					"userCode": "ABCD-1234",
					"verificationUri": "https://www.kimi.com/code?user_code=ABCD-1234",
					"intervalSeconds": 5.0,
					"expiresInSeconds": 600.0,
				})]
			);
			// waitBeforeFirstPoll: first poll happens after the 5s interval.
			assert_eq!(*poll_times.borrow(), vec![start + 5000.0, start + 10_000.0]);
			assert_eq!(
				credential_json(&credential),
				json!({
					"type": "oauth",
					"refresh": "refresh-token",
					"access": "access-token",
					"expires": start + 10_000.0 + 3600.0 * 1000.0,
					"oauthHost": OAUTH_HOST,
					"baseUrl": "https://api.kimi.com/coding/v1",
				})
			);
		});
	}

	fn fails_with_token_error(error: &'static str, expected: &str) {
		let fetch = handler(move |request| {
			let url = request.url.as_str();
			if url == format!("{OAUTH_HOST}/api/oauth/device_authorization") {
				return Ok(device_authorization_response(json!({})));
			}
			if url == format!("{OAUTH_HOST}/api/oauth/token") {
				return Ok(json_response(json!({ "error": error }), 400));
			}
			Err(PiError::new(format!("Unexpected fetch URL: {url}")))
		});
		run_with_env_and_fake_timers(fetch, &EMPTY_ENV, || async {
			let failure = kimi_coding_oauth()
				.login(create_interaction(no_events(), "mainland-cn"), None)
				.await
				.unwrap_err();
			assert!(failure.message.contains(expected), "{}", failure.message);
		});
	}

	#[test]
	fn fails_when_the_device_code_expires() {
		fails_with_token_error("expired_token", "expired");
	}

	#[test]
	fn fails_when_the_user_denies_the_login() {
		fails_with_token_error("access_denied", "denied");
	}

	#[test]
	fn honors_the_kimi_code_oauth_host_override() {
		let urls: Rc<RefCell<Vec<String>>> = Rc::new(RefCell::new(Vec::new()));
		let recorded = urls.clone();
		let fetch = handler(move |request| {
			let url = request.url.clone();
			recorded.borrow_mut().push(url.clone());
			if url == "https://auth.example.com/api/oauth/device_authorization" {
				return Ok(device_authorization_response(json!({ "interval": 1 })));
			}
			if url == "https://auth.example.com/api/oauth/token" {
				return Ok(json_response(
					json!({ "access_token": "a", "refresh_token": "r", "expires_in": 60 }),
					200,
				));
			}
			Err(PiError::new(format!("Unexpected fetch URL: {url}")))
		});
		run_with_env_and_fake_timers(
			fetch,
			&env_with("KIMI_CODE_OAUTH_HOST", "https://auth.example.com/"),
			|| async {
				let credential = kimi_coding_oauth()
					.login(create_interaction(no_events(), "mainland-cn"), None)
					.await
					.unwrap();
				assert_matches(&credential_json(&credential), json!({ "access": "a", "refresh": "r" }));
				assert_eq!(
					*urls.borrow(),
					vec![
						"https://auth.example.com/api/oauth/device_authorization",
						"https://auth.example.com/api/oauth/token",
					]
				);
			},
		);
	}

	#[test]
	fn refreshes_tokens_and_returns_a_bearer_header_for_requests() {
		let fetch = handler(|request| {
			assert_eq!(request.url, format!("{OAUTH_HOST}/api/oauth/token"));
			let params = form_body(&request);
			assert_eq!(params["grant_type"], "refresh_token");
			assert_eq!(params["refresh_token"], "old-refresh");
			assert_eq!(params["client_id"], CLIENT_ID);
			Ok(json_response(
				json!({ "access_token": "new-access", "refresh_token": "new-refresh", "expires_in": 3600 }),
				200,
			))
		});
		run_with_env_and_fake_timers(fetch, &EMPTY_ENV, || async {
			let before = pi_host::now_ms();
			let oauth = kimi_coding_oauth();
			let credential = oauth
				.refresh(
					OAuthCredential::new("old-refresh", "old-access", before),
					AbortController::new().signal(),
				)
				.await
				.unwrap();
			let stored = credential_json(&credential);
			assert_matches(
				&stored,
				json!({
					"type": "oauth",
					"oauthHost": OAUTH_HOST,
					"baseUrl": "https://api.kimi.com/coding/v1",
					"access": "new-access",
					"refresh": "new-refresh",
				}),
			);
			assert_eq!(stored.as_object().unwrap().len(), 6);
			assert!(credential.expires >= before + 3600.0 * 1000.0);

			let mut headers = indexmap::IndexMap::new();
			headers.insert("Authorization".to_string(), Some("Bearer new-access".to_string()));
			assert_eq!(
				oauth.to_auth(credential).await.unwrap(),
				ModelAuth {
					api_key: None,
					headers: Some(headers),
					base_url: Some("https://api.kimi.com/coding/v1".into()),
				}
			);
		});
	}

	// Same regional-login failure reported in can1357/oh-my-pi#12024.
	#[test]
	fn keeps_international_login_refresh_and_model_requests_on_kimi_ai_after_environment_changes() {
		let urls: Rc<RefCell<Vec<String>>> = Rc::new(RefCell::new(Vec::new()));
		let recorded = urls.clone();
		let fetch = handler(move |request| {
			let url = request.url.clone();
			recorded.borrow_mut().push(url.clone());
			if url == "https://auth.kimi.ai/api/oauth/device_authorization" {
				return Ok(device_authorization_response(json!({
					"interval": 1,
					"verification_uri": "https://www.kimi.ai/code/authorize_device",
					"verification_uri_complete": "https://www.kimi.ai/code/authorize_device?user_code=ABCD-1234",
				})));
			}
			if url == "https://auth.kimi.ai/api/oauth/token" {
				return Ok(json_response(
					json!({ "access_token": "global-access", "refresh_token": "global-refresh", "expires_in": 3600 }),
					200,
				));
			}
			Err(PiError::new(format!("Unexpected fetch URL: {url}")))
		});
		let credential = run_with_env_and_fake_timers(fetch.clone(), &EMPTY_ENV, || async {
			let events = no_events();
			let credential = kimi_coding_oauth()
				.login(create_interaction(events.clone(), "global"), None)
				.await
				.unwrap();
			assert_eq!(
				events.borrow()[0]["verificationUri"],
				"https://www.kimi.ai/code/authorize_device?user_code=ABCD-1234"
			);
			assert_matches(
				&credential_json(&credential),
				json!({ "oauthHost": "https://auth.kimi.ai", "baseUrl": "https://api.kimi.ai/coding/v1" }),
			);
			credential
		});
		let changed_env = [
			("KIMI_CODE_OAUTH_HOST", OAUTH_HOST),
			("KIMI_OAUTH_HOST", ""),
			("KIMI_CODE_BASE_URL", "https://api.kimi.com/coding"),
		];
		run_with_env_and_fake_timers(fetch, &changed_env, || async {
			// Round-trip through storage as a restarted client would.
			let stored: OAuthCredential = serde_json::from_value(credential_json(&credential)).unwrap();
			let refreshed = kimi_coding_oauth()
				.refresh(stored, AbortController::new().signal())
				.await
				.unwrap();
			assert_matches(
				&credential_json(&refreshed),
				json!({ "oauthHost": "https://auth.kimi.ai", "baseUrl": "https://api.kimi.ai/coding/v1" }),
			);

			let credentials = Rc::new(InMemoryCredentialStore::new());
			credentials
				.modify(
					"kimi-coding",
					Box::new(move |_| Box::pin(async move { Ok(Some(Credential::OAuth(refreshed))) })),
					None,
				)
				.await
				.unwrap();
			let models = create_models(Some(CreateModelsOptions {
				credentials: Some(credentials),
				..CreateModelsOptions::default()
			}));
			models.set_provider(Rc::new(kimi_coding_provider()));
			let requests = Rc::new(Cell::new(0));
			let counted = requests.clone();
			let mut options = ModelsSimpleStreamOptions::default();
			options.fetch = Some(Rc::new(move |request: HttpRequest| {
				counted.set(counted.get() + 1);
				assert_eq!(request.url, "https://api.kimi.ai/coding/v1/chat/completions");
				assert_eq!(request.header("Authorization"), Some("Bearer global-access"));
				let response = HttpResponse::from_bytes(
					200,
					IndexMap::from([("content-type".to_string(), "text/event-stream".to_string())]),
					"data: {\"id\":\"chat_test\",\"choices\":[{\"index\":0,\"delta\":{\"content\":\"Hello\"},\"finish_reason\":\"stop\"}]}\n\ndata: [DONE]\n\n",
				);
				Box::pin(async move { Ok(response) })
			}));
			let result = models
				.complete_simple(
					&models.get_model("kimi-coding", "kimi-for-coding").unwrap(),
					&Context {
						messages: vec![Message::User(UserMessage {
							content: UserMessageContent::Text("Hello".into()),
							timestamp: pi_host::now_ms(),
							..UserMessage::default()
						})],
						..Context::default()
					},
					Some(options),
				)
				.await
				.unwrap();
			assert_eq!(result.error_message, None);
			assert_eq!(result.stop_reason, StopReason::Stop);
			assert_eq!(requests.get(), 1);
		});
		assert_eq!(
			*urls.borrow(),
			vec![
				"https://auth.kimi.ai/api/oauth/device_authorization",
				"https://auth.kimi.ai/api/oauth/token",
				"https://auth.kimi.ai/api/oauth/token",
			]
		);
	}

	#[test]
	fn infers_both_international_endpoints_from_an_override_without_a_prompt() {
		for (name, value) in [
			("KIMI_CODE_OAUTH_HOST", "https://auth.kimi.ai/"),
			("KIMI_OAUTH_HOST", "https://auth.kimi.ai"),
			("KIMI_CODE_BASE_URL", "https://api.kimi.ai/coding/v1/"),
		] {
			let fetch = handler(|request| {
				let url = request.url.as_str();
				if url == "https://auth.kimi.ai/api/oauth/device_authorization" {
					return Ok(device_authorization_response(json!({ "interval": 1 })));
				}
				if url == "https://auth.kimi.ai/api/oauth/token" {
					return Ok(json_response(
						json!({ "access_token": "a", "refresh_token": "r", "expires_in": 60 }),
						200,
					));
				}
				Err(PiError::new(format!("Unexpected fetch URL: {url}")))
			});
			run_with_env_and_fake_timers(fetch, &env_with(name, value), || async {
				let prompted = Rc::new(Cell::new(false));
				let flag = prompted.clone();
				let interaction = with_prompt(no_events(), move |_| {
					flag.set(true);
					Box::pin(async { Err(PiError::new("Should not prompt")) })
				});
				let credential = kimi_coding_oauth().login(interaction, None).await.unwrap();
				assert_matches(
					&credential_json(&credential),
					json!({ "oauthHost": "https://auth.kimi.ai", "baseUrl": "https://api.kimi.ai/coding/v1" }),
				);
				assert!(!prompted.get(), "{name}");
			});
		}
	}

	#[test]
	fn does_not_start_device_authorization_when_region_selection_is_canceled() {
		let calls = Rc::new(Cell::new(0));
		let counted = calls.clone();
		let fetch = handler(move |_| {
			counted.set(counted.get() + 1);
			Err(PiError::new("fetch must not run"))
		});
		run_with_env_and_fake_timers(fetch, &EMPTY_ENV, || async {
			let interaction = with_prompt(no_events(), |_| Box::pin(async { Err(PiError::new("Canceled")) }));
			let error = kimi_coding_oauth().login(interaction, None).await.unwrap_err();
			assert!(error.message.contains("Canceled"));
			assert_eq!(calls.get(), 0);
		});
	}

	#[test]
	fn retries_refresh_on_429_and_fails_unauthorized_on_invalid_grant() {
		// 429 once, then success.
		let calls = Rc::new(Cell::new(0));
		let counted = calls.clone();
		let fetch = handler(move |_| {
			counted.set(counted.get() + 1);
			Ok(if counted.get() == 1 {
				json_response(json!({ "error": "temporarily_unavailable" }), 429)
			} else {
				json_response(
					json!({ "access_token": "a", "refresh_token": "r", "expires_in": 60 }),
					200,
				)
			})
		});
		run_with_env_and_fake_timers(fetch, &EMPTY_ENV, || async {
			let start = pi_host::now_ms();
			let refreshed = kimi_coding_oauth()
				.refresh(OAuthCredential::new("old", "old", 0.0), AbortController::new().signal())
				.await
				.unwrap();
			assert_eq!(refreshed.access, "a");
			assert_eq!(pi_host::now_ms() - start, 1000.0);
		});
		assert_eq!(calls.get(), 2);

		// invalid_grant is not retried.
		let fetch = handler(|_| Ok(json_response(json!({ "error": "invalid_grant" }), 400)));
		run_with_env_and_fake_timers(fetch, &EMPTY_ENV, || async {
			let error = kimi_coding_oauth()
				.refresh(OAuthCredential::new("old", "old", 0.0), AbortController::new().signal())
				.await
				.unwrap_err();
			assert!(error.message.contains("unauthorized"));
		});
	}
}
