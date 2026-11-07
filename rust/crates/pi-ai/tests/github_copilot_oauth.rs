//! Port of `packages/ai/test/github-copilot-oauth.test.ts`.
//!
//! The tests run on `pi_host::testing`'s fake timers, which advance to the next timer whenever every task waits, so
//! they check the recorded request times instead of stepping the clock. `githubCopilotProvider().getModels()` lists
//! `GITHUB_COPILOT_MODELS` in order, so the tests read model ids from that catalog
//! (`providers/github_copilot_models.rs`, w2-ai-core) and wait for it. The `createModels` steps (reading the
//! credential back through `Models.getAvailable`, and logging in through `Models.login` into a credential store)
//! need w2-ai-core's `models.rs` and are not ported; the tests check the credential the flow returns instead.

mod common;

use std::cell::RefCell;
use std::collections::VecDeque;
use std::rc::Rc;

use common::{FetchHandler, form_body, json_response};
use indexmap::IndexMap;
use pi_ai::auth::oauth::github_copilot::github_copilot_oauth;
use pi_ai::auth::types::{AuthEvent, AuthPromptKind, OAuthCredential, ProviderAuthInteraction};
use pi_ai::providers::github_copilot_models::GITHUB_COPILOT_MODELS;
use pi_host::{AbortController, HttpRequest, HttpResponse, PiError, PiResult};
use serde_json::{Value, json};

const TEST_COPILOT_ACCESS_TOKEN: &str = "tid=test;exp=9999999999;proxy-ep=proxy.individual.githubcopilot.com;";
const TEST_COPILOT_MODELS_URL: &str = "https://api.individual.githubcopilot.com/models";

fn handler(respond: impl Fn(HttpRequest) -> PiResult<HttpResponse> + 'static) -> FetchHandler {
	Rc::new(move |request| {
		let response = respond(request);
		Box::pin(async move { response })
	})
}

fn json_response_with_headers(body: Value, status: u16, headers: &[(&str, &str)]) -> HttpResponse {
	let mut response = json_response(body, status);
	for (name, value) in headers {
		response.headers.insert(name.to_ascii_lowercase(), value.to_string());
	}
	response
}

fn empty_response() -> HttpResponse {
	HttpResponse::from_bytes(200, IndexMap::new(), "")
}

fn header<'a>(request: &'a HttpRequest, name: &str) -> Option<&'a str> {
	request.headers.get(name).map(String::as_str)
}

fn request_body(request: &HttpRequest) -> String {
	String::from_utf8(request.body.as_deref().unwrap_or_default().to_vec()).unwrap()
}

fn require_model_id(index: usize) -> String {
	GITHUB_COPILOT_MODELS
		.keys()
		.nth(index)
		.unwrap_or_else(|| panic!("Expected a GitHub Copilot model at index {index}"))
		.clone()
}

fn device_code_response(verification_uri: &str, interval: f64, expires_in: f64) -> HttpResponse {
	json_response(
		json!({
			"device_code": "device-code",
			"user_code": "ABCD-EFGH",
			"verification_uri": verification_uri,
			"interval": interval,
			"expires_in": expires_in,
		}),
		200,
	)
}

fn copilot_token_response() -> HttpResponse {
	json_response(
		json!({ "token": TEST_COPILOT_ACCESS_TOKEN, "expires_at": 9_999_999_999u64 }),
		200,
	)
}

/// The responses of a login that `stubGitHubCopilotLoginFetch` does not configure.
fn default_login_response(url: &str) -> Option<PiResult<HttpResponse>> {
	if url.ends_with("/login/device/code") {
		return Some(Ok(device_code_response("https://github.com/login/device", 1.0, 900.0)));
	}
	if url.ends_with("/login/oauth/access_token") {
		return Some(Ok(json_response(json!({ "access_token": "ghu_refresh_token" }), 200)));
	}
	if url.contains("/copilot_internal/v2/token") {
		return Some(Ok(copilot_token_response()));
	}
	None
}

type PolicyHandler = Rc<dyn Fn(&str) -> PiResult<HttpResponse>>;

fn stub_github_copilot_login_fetch(
	models: impl Fn() -> HttpResponse + 'static,
	policy: Option<PolicyHandler>,
) -> FetchHandler {
	handler(move |request| {
		let url = request.url.clone();
		if let Some(response) = default_login_response(&url) {
			return response;
		}
		if url == TEST_COPILOT_MODELS_URL {
			return Ok(models());
		}
		let prefix = format!("{TEST_COPILOT_MODELS_URL}/");
		if let Some(model_id) = url.strip_prefix(&prefix).and_then(|rest| rest.strip_suffix("/policy")) {
			let Some(policy) = &policy else {
				return Err(PiError::new(format!("Unexpected policy request: {url}")));
			};
			return policy(model_id);
		}
		Err(PiError::new(format!("Unexpected fetch URL: {url}")))
	})
}

/// `loginGitHubCopilotForTest`: answers the domain prompt with `""` and records the device-code and progress events.
async fn login_github_copilot_for_test(events: Rc<RefCell<Vec<Value>>>) -> PiResult<OAuthCredential> {
	github_copilot_oauth()
		.login(
			ProviderAuthInteraction {
				signal: AbortController::new().signal(),
				prompt: Rc::new(|prompt| {
					Box::pin(async move {
						if !matches!(prompt.kind, AuthPromptKind::Text { .. }) {
							return Err(PiError::new(format!("Unexpected prompt: {}", prompt.type_name())));
						}
						Ok(String::new())
					})
				}),
				notify: Rc::new(move |event| {
					if matches!(event, AuthEvent::DeviceCode { .. } | AuthEvent::Progress { .. }) {
						events.borrow_mut().push(serde_json::to_value(event).unwrap());
					}
				}),
			},
			None,
		)
		.await
}

fn device_code_events(events: &[Value]) -> Vec<Value> {
	events
		.iter()
		.filter(|event| event["type"] == "device_code")
		.cloned()
		.collect()
}

fn refresh_github_copilot_models_for_test(data: Value, proxy_host: &str) -> PiResult<OAuthCredential> {
	let access_token = format!("tid=test;exp=9999999999;proxy-ep={proxy_host};");
	let models_url = format!(
		"https://{}/models",
		proxy_host
			.strip_prefix("proxy.")
			.map_or(proxy_host.to_string(), |rest| format!("api.{rest}"))
	);
	let token = access_token.clone();
	common::run_with_fake_timers(
		handler(move |request| {
			if request.url.contains("/copilot_internal/v2/token") {
				return Ok(json_response(
					json!({ "token": token, "expires_at": 9_999_999_999u64 }),
					200,
				));
			}
			if request.url == models_url {
				assert_eq!(
					header(&request, "Authorization"),
					Some(format!("Bearer {token}").as_str())
				);
				return Ok(json_response(json!({ "data": data }), 200));
			}
			Err(PiError::new(format!("Unexpected fetch URL: {}", request.url)))
		}),
		|| {
			github_copilot_oauth().refresh(
				OAuthCredential::new("ghu_refresh_token", "old-access-token", 0.0),
				AbortController::new().signal(),
			)
		},
	)
}

fn tool_capable_model(id: &str, picker_enabled: bool, policy_state: Option<&str>, tool_calls: bool) -> Value {
	let mut model = json!({
		"id": id,
		"model_picker_enabled": picker_enabled,
		"capabilities": { "supports": { "tool_calls": tool_calls } },
	});
	if let Some(policy_state) = policy_state {
		model["policy"] = json!({ "state": policy_state });
	}
	model
}

mod git_hub_copilot_o_auth_device_flow {
	use super::*;

	#[test]
	#[ignore = "PORT-STUB: needs providers/github_copilot_models.rs from w2-ai-core"]
	fn filters_models_to_the_authenticated_account_picker_catalog() {
		let picker_model_id = require_model_id(0);
		let disabled_model_id = require_model_id(1);
		let hidden_model_id = require_model_id(2);

		let credentials = refresh_github_copilot_models_for_test(
			json!([
				tool_capable_model(&picker_model_id, true, None, true),
				tool_capable_model(&disabled_model_id, true, Some("disabled"), true),
				tool_capable_model(&hidden_model_id, false, Some("enabled"), true),
			]),
			"proxy.individual.githubcopilot.com",
		)
		.unwrap();
		assert_eq!(credentials.field("availableModelIds"), Some(&json!([picker_model_id])));
	}

	#[test]
	#[ignore = "PORT-STUB: needs providers/github_copilot_models.rs from w2-ai-core"]
	fn falls_back_to_explicitly_enabled_policy_models_when_the_picker_catalog_is_empty() {
		let enabled_model_id = require_model_id(0);
		let credentials = refresh_github_copilot_models_for_test(
			json!([
				tool_capable_model(&enabled_model_id, false, Some("enabled"), true),
				tool_capable_model("policy-disabled-model", false, Some("disabled"), true),
				tool_capable_model("unconfigured-model", false, None, true),
				tool_capable_model("tool-incapable-model", false, Some("enabled"), false),
			]),
			"proxy.individual.githubcopilot.com",
		)
		.unwrap();

		assert_eq!(credentials.field("availableModelIds"), Some(&json!([enabled_model_id])));
	}

	#[test]
	fn does_not_fall_back_to_policy_models_for_non_individual_accounts() {
		let credentials = refresh_github_copilot_models_for_test(
			json!([tool_capable_model("gpt-4.1", false, Some("enabled"), true)]),
			"proxy.business.githubcopilot.com",
		)
		.unwrap();

		assert_eq!(credentials.field("availableModelIds"), Some(&json!([])));
	}

	#[test]
	fn does_not_retry_model_catalog_throttling_during_credential_refresh() {
		let catalog_request_count = Rc::new(RefCell::new(0));
		let count = catalog_request_count.clone();
		let error = common::run_with_fake_timers(
			handler(move |request| {
				if request.url.contains("/copilot_internal/v2/token") {
					return Ok(copilot_token_response());
				}
				if request.url == TEST_COPILOT_MODELS_URL {
					*count.borrow_mut() += 1;
					return Ok(json_response_with_headers(
						json!({ "error": "too many requests" }),
						429,
						&[("Retry-After", "0")],
					));
				}
				Err(PiError::new(format!("Unexpected fetch URL: {}", request.url)))
			}),
			|| {
				github_copilot_oauth().refresh(
					OAuthCredential::new("ghu_refresh_token", "old-access-token", 0.0),
					AbortController::new().signal(),
				)
			},
		)
		.unwrap_err();

		assert!(error.message.contains("429"), "{}", error.message);
		assert_eq!(*catalog_request_count.borrow(), 1);
	}

	#[test]
	fn reports_device_code_details_through_on_device_code() {
		let fetch = handler(|request| {
			let url = request.url.as_str();
			if let Some(response) = default_login_response(url) {
				return response;
			}
			if url.ends_with("/models") {
				return Ok(json_response(json!({ "data": [] }), 200));
			}
			if url.contains("/models/") && url.ends_with("/policy") {
				return Ok(empty_response());
			}
			Err(PiError::new(format!("Unexpected fetch URL: {url}")))
		});
		let events = Rc::new(RefCell::new(Vec::new()));
		common::run_with_fake_timers(fetch, || login_github_copilot_for_test(events.clone())).unwrap();

		assert_eq!(
			device_code_events(&events.borrow()),
			vec![json!({
				"type": "device_code",
				"userCode": "ABCD-EFGH",
				"verificationUri": "https://github.com/login/device",
				"intervalSeconds": 1.0,
				"expiresInSeconds": 900.0,
			})]
		);
	}

	#[test]
	#[ignore = "PORT-STUB: needs providers/github_copilot_models.rs from w2-ai-core"]
	fn updates_only_known_tool_capable_unconfigured_account_model_policies() {
		let configured_model_id = require_model_id(0);
		let unconfigured_model_id = require_model_id(1);
		let tool_incapable_model_id = require_model_id(2);
		let catalog_request_count = Rc::new(RefCell::new(0));
		let policy_model_ids = Rc::new(RefCell::new(Vec::<String>::new()));
		let count = catalog_request_count.clone();
		let policies = policy_model_ids.clone();
		let data = json!([
			tool_capable_model(&configured_model_id, true, Some("enabled"), true),
			tool_capable_model(&unconfigured_model_id, true, Some("unconfigured"), true),
			tool_capable_model("remote-only-model", true, Some("unconfigured"), true),
			tool_capable_model(&tool_incapable_model_id, true, Some("unconfigured"), false),
		]);
		let fetch = stub_github_copilot_login_fetch(
			move || {
				*count.borrow_mut() += 1;
				json_response(json!({ "data": data }), 200)
			},
			Some(Rc::new(move |model_id: &str| {
				policies.borrow_mut().push(model_id.to_string());
				Ok(empty_response())
			})),
		);

		common::run_with_fake_timers(fetch, || login_github_copilot_for_test(Rc::default())).unwrap();

		assert_eq!(*catalog_request_count.borrow(), 1);
		assert_eq!(*policy_model_ids.borrow(), vec![unconfigured_model_id]);
	}

	#[test]
	#[ignore = "PORT-STUB: needs providers/github_copilot_models.rs from w2-ai-core"]
	fn retries_a_throttled_policy_update_after_retry_after() {
		let model_id = require_model_id(0);
		let policy_request_times = Rc::new(RefCell::new(Vec::<f64>::new()));
		let times = policy_request_times.clone();
		let data = json!([{ "id": model_id, "model_picker_enabled": true, "policy": { "state": "unconfigured" } }]);
		let fetch = stub_github_copilot_login_fetch(
			move || json_response(json!({ "data": data }), 200),
			Some(Rc::new(move |_: &str| {
				times.borrow_mut().push(pi_host::now_ms());
				Ok(if times.borrow().len() == 1 {
					json_response_with_headers(json!({ "error": "too many requests" }), 429, &[("Retry-After", "1")])
				} else {
					empty_response()
				})
			})),
		);

		let start = Rc::new(RefCell::new(0.0));
		let started = start.clone();
		common::run_with_fake_timers(fetch, || {
			*started.borrow_mut() = pi_host::now_ms();
			login_github_copilot_for_test(Rc::default())
		})
		.unwrap();

		// The first policy request follows the one-second poll wait; the retry waits the Retry-After second.
		let start = *start.borrow();
		assert_eq!(*policy_request_times.borrow(), vec![start + 1000.0, start + 2000.0]);
	}

	#[test]
	#[ignore = "PORT-STUB: needs providers/github_copilot_models.rs from w2-ai-core"]
	fn continues_policy_updates_after_a_transport_failure() {
		let model_ids = vec![require_model_id(0), require_model_id(1)];
		let policy_model_ids = Rc::new(RefCell::new(Vec::<String>::new()));
		let policies = policy_model_ids.clone();
		let data = Value::Array(
			model_ids
				.iter()
				.map(|id| json!({ "id": id, "model_picker_enabled": true, "policy": { "state": "unconfigured" } }))
				.collect(),
		);
		let fetch = stub_github_copilot_login_fetch(
			move || json_response(json!({ "data": data }), 200),
			Some(Rc::new(move |model_id: &str| {
				policies.borrow_mut().push(model_id.to_string());
				if policies.borrow().len() == 1 {
					return Err(PiError::type_error("fetch failed"));
				}
				Ok(empty_response())
			})),
		);

		common::run_with_fake_timers(fetch, || login_github_copilot_for_test(Rc::default())).unwrap();

		assert_eq!(*policy_model_ids.borrow(), model_ids);
	}

	#[test]
	#[ignore = "PORT-STUB: needs providers/github_copilot_models.rs from w2-ai-core"]
	fn stops_policy_updates_and_persists_authentication_when_the_retry_delay_exceeds_the_login_budget() {
		let first_model_id = require_model_id(0);
		let second_model_id = require_model_id(1);
		let policy_model_ids = Rc::new(RefCell::new(Vec::<String>::new()));
		let policies = policy_model_ids.clone();
		let data = json!([
			{ "id": first_model_id, "model_picker_enabled": true, "policy": { "state": "unconfigured" } },
			{ "id": second_model_id, "model_picker_enabled": true, "policy": { "state": "unconfigured" } },
		]);
		let fetch = stub_github_copilot_login_fetch(
			move || json_response(json!({ "data": data }), 200),
			Some(Rc::new(move |model_id: &str| {
				policies.borrow_mut().push(model_id.to_string());
				Ok(json_response_with_headers(
					json!({ "error": "too many requests" }),
					429,
					&[("Retry-After", "5")],
				))
			})),
		);

		let credential = common::run_with_fake_timers(fetch, || login_github_copilot_for_test(Rc::default())).unwrap();
		assert_eq!(credential.access, TEST_COPILOT_ACCESS_TOKEN);
		assert_eq!(*policy_model_ids.borrow(), vec![first_model_id]);
	}

	#[test]
	fn rejects_a_non_http_s_verification_uri_before_it_reaches_on_device_code() {
		// A malicious enterprise OAuth server could return a verification_uri that
		// the browser launcher would otherwise hand to the OS. Ensure such values
		// are rejected at the deserialization boundary.
		let fetch = handler(|request| {
			if request.url.ends_with("/login/device/code") {
				return Ok(device_code_response("$(id>/tmp/pwned)", 1.0, 900.0));
			}
			Err(PiError::new(format!("Unexpected fetch URL: {}", request.url)))
		});
		let events = Rc::new(RefCell::new(Vec::new()));
		let error = common::run_with_fake_timers(fetch, || login_github_copilot_for_test(events.clone())).unwrap_err();

		assert!(
			error.message.contains("Untrusted verification_uri"),
			"{}",
			error.message
		);
		assert!(device_code_events(&events.borrow()).is_empty());
	}

	#[test]
	fn normalizes_verification_uri_before_it_reaches_on_device_code() {
		let raw_verification_uri = "https://github.com/login/\x1b]8;;evil";
		let normalized_verification_uri = url::Url::parse(raw_verification_uri).unwrap().to_string();
		assert_ne!(normalized_verification_uri, raw_verification_uri);

		let fetch = handler(move |request| {
			let url = request.url.as_str();
			if url.ends_with("/login/device/code") {
				return Ok(device_code_response(raw_verification_uri, 1.0, 900.0));
			}
			if let Some(response) = default_login_response(url) {
				return response;
			}
			if url.ends_with("/models") {
				return Ok(json_response(json!({ "data": [] }), 200));
			}
			if url.contains("/models/") && url.ends_with("/policy") {
				return Ok(empty_response());
			}
			Err(PiError::new(format!("Unexpected fetch URL: {url}")))
		});
		let events = Rc::new(RefCell::new(Vec::new()));
		common::run_with_fake_timers(fetch, || login_github_copilot_for_test(events.clone())).unwrap();

		assert_eq!(
			device_code_events(&events.borrow()),
			vec![json!({
				"type": "device_code",
				"userCode": "ABCD-EFGH",
				"verificationUri": normalized_verification_uri,
				"intervalSeconds": 1.0,
				"expiresInSeconds": 900.0,
			})]
		);
	}

	#[test]
	fn waits_before_polling_and_increases_the_interval_after_slow_down() {
		let access_token_poll_times = Rc::new(RefCell::new(Vec::<f64>::new()));
		let access_token_responses = Rc::new(RefCell::new(VecDeque::from([
			json_response(
				json!({ "error": "authorization_pending", "error_description": "pending" }),
				200,
			),
			json_response(
				json!({ "error": "slow_down", "error_description": "slow down", "interval": 7 }),
				200,
			),
			json_response(json!({ "access_token": "ghu_refresh_token" }), 200),
		])));
		let poll_times = access_token_poll_times.clone();
		let fetch = handler(move |request| {
			let url = request.url.as_str();
			if url.ends_with("/login/device/code") {
				assert_eq!(request.method, "POST");
				assert_eq!(header(&request, "Accept"), Some("application/json"));
				assert_eq!(
					header(&request, "Content-Type"),
					Some("application/x-www-form-urlencoded")
				);
				assert!(request_body(&request).contains("client_id="));
				assert!(request_body(&request).contains("scope=read%3Auser"));
				return Ok(device_code_response("https://github.com/login/device", 5.0, 900.0));
			}
			if url.ends_with("/login/oauth/access_token") {
				poll_times.borrow_mut().push(pi_host::now_ms());
				assert_eq!(request.method, "POST");
				assert_eq!(header(&request, "Accept"), Some("application/json"));
				assert_eq!(
					header(&request, "Content-Type"),
					Some("application/x-www-form-urlencoded")
				);
				let body = request_body(&request);
				assert!(body.contains("client_id="));
				assert!(body.contains("device_code=device-code"));
				assert!(body.contains("grant_type=urn%3Aietf%3Aparams%3Aoauth%3Agrant-type%3Adevice_code"));
				assert_eq!(form_body(&request)["device_code"], "device-code");
				return access_token_responses
					.borrow_mut()
					.pop_front()
					.ok_or_else(|| PiError::new("Unexpected extra access token poll"));
			}
			if url.contains("/copilot_internal/v2/token") {
				return Ok(copilot_token_response());
			}
			if url.ends_with("/models") {
				return Ok(json_response(json!({ "data": [] }), 200));
			}
			if url.contains("/models/") && url.ends_with("/policy") {
				return Ok(empty_response());
			}
			Err(PiError::new(format!("Unexpected fetch URL: {url}")))
		});

		let start = Rc::new(RefCell::new(0.0));
		let started = start.clone();
		common::run_with_fake_timers(fetch, || {
			*started.borrow_mut() = pi_host::now_ms();
			login_github_copilot_for_test(Rc::default())
		})
		.unwrap();

		// slow_down carried a server-provided interval of 7 seconds.
		let start = *start.borrow();
		assert_eq!(
			*access_token_poll_times.borrow(),
			vec![start + 5000.0, start + 10_000.0, start + 17_000.0]
		);
	}

	#[test]
	fn times_out_after_repeated_slow_down_responses() {
		let access_token_poll_times = Rc::new(RefCell::new(Vec::<f64>::new()));
		let access_token_responses = Rc::new(RefCell::new(VecDeque::from([
			json_response(json!({ "error": "slow_down", "error_description": "slow down" }), 200),
			json_response(
				json!({ "error": "slow_down", "error_description": "still too fast" }),
				200,
			),
			json_response(
				json!({ "error": "authorization_pending", "error_description": "pending" }),
				200,
			),
		])));
		let poll_times = access_token_poll_times.clone();
		let fetch = handler(move |request| {
			let url = request.url.as_str();
			if url.ends_with("/login/device/code") {
				return Ok(device_code_response("https://github.com/login/device", 5.0, 25.0));
			}
			if url.ends_with("/login/oauth/access_token") {
				poll_times.borrow_mut().push(pi_host::now_ms());
				return access_token_responses
					.borrow_mut()
					.pop_front()
					.ok_or_else(|| PiError::new("Unexpected extra access token poll"));
			}
			Err(PiError::new(format!("Unexpected fetch URL: {url}")))
		});

		let start = Rc::new(RefCell::new(0.0));
		let started = start.clone();
		let error = common::run_with_fake_timers(fetch, || {
			*started.borrow_mut() = pi_host::now_ms();
			login_github_copilot_for_test(Rc::default())
		})
		.unwrap_err();

		assert!(
			error
				.message
				.contains("Device flow timed out after one or more slow_down responses"),
			"{}",
			error.message
		);
		let start = *start.borrow();
		assert_eq!(
			*access_token_poll_times.borrow(),
			vec![start + 5000.0, start + 15_000.0]
		);
	}
}
