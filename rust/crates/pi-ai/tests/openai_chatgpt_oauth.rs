//! Port of `packages/ai/test/openai-chatgpt-oauth.test.ts`.
//!
//! The login tests bind the fixed callback port 1455, so they need the `native` feature and hold
//! `common::sequential()`. The device ID and refresh tests run without either.

mod common;

use std::cell::RefCell;
use std::rc::Rc;

use common::{form_body, json_response};
use indexmap::IndexMap;
use pi_ai::auth::oauth::openai_chatgpt::openai_chat_gpt_oauth;
use pi_ai::auth::types::{AuthEvent, AuthPromptKind, LoginOptions, OAuthCredential, ProviderAuthInteraction};
use pi_host::{AbortController, PiError};
use serde_json::{Value, json};
use url::Url;

const TOKEN_URL: &str = "https://auth.openai.com/api/accounts/oauth/token";
const REQUIRED_SCOPE: &str = "openid profile email offline_access resource.invoke chatgpt.tokens.use.direct";

type RecordedBody = Rc<RefCell<Option<IndexMap<String, String>>>>;

fn token_response(scope: &str) -> Value {
	json!({
		"access_token": "access-token",
		"refresh_token": "refresh-token",
		"expires_in": 3600,
		"id_token": "id-token",
		"scope": scope,
	})
}

/// `stubTokenEndpoint`: answers every request with `response`, records its form body and counts the calls.
fn stub_token_endpoint(response: Value, body: RecordedBody, calls: Rc<RefCell<usize>>) -> common::FetchHandler {
	Rc::new(move |request| {
		assert_eq!(request.url, TOKEN_URL);
		*body.borrow_mut() = Some(form_body(&request));
		*calls.borrow_mut() += 1;
		let response = response.clone();
		Box::pin(async move { Ok(json_response(response, 200)) })
	})
}

fn search_param(url: &Url, name: &str) -> Option<String> {
	url.query_pairs()
		.find(|(key, _)| key == name)
		.map(|(_, value)| value.into_owned())
}

fn login_interaction(
	callback_client_id: Option<&'static str>,
	authorize_url: Rc<RefCell<Option<Url>>>,
) -> ProviderAuthInteraction {
	let notified = authorize_url.clone();
	ProviderAuthInteraction {
		signal: AbortController::new().signal(),
		notify: Rc::new(move |event| {
			if let AuthEvent::AuthUrl { url, .. } = event {
				*notified.borrow_mut() = Some(Url::parse(&url).unwrap());
			}
		}),
		prompt: Rc::new(move |prompt| {
			let authorize_url = authorize_url.borrow().clone();
			Box::pin(async move {
				if !matches!(prompt.kind, AuthPromptKind::ManualCode { .. }) {
					return Err(PiError::new(format!("Unexpected prompt: {}", prompt.type_name())));
				}
				let Some(authorize_url) = authorize_url else {
					return Err(PiError::new(
						"Authorization URL was not emitted before the callback prompt",
					));
				};
				let mut callback = Url::parse(&search_param(&authorize_url, "redirect_uri").unwrap_or_default())
					.map_err(|error| PiError::type_error(format!("Invalid URL: {error}")))?;
				{
					let mut query = callback.query_pairs_mut();
					query.append_pair("code", "authorization-code");
					query.append_pair("state", &search_param(&authorize_url, "state").unwrap_or_default());
					if let Some(client_id) = callback_client_id {
						query.append_pair("client_id", client_id);
					}
				}
				Ok(callback.to_string())
			})
		}),
	}
}

fn connected_credential() -> OAuthCredential {
	let mut credential = OAuthCredential::new("old-refresh", "old-access", 0.0);
	credential.set_field("clientId", json!("oaiapp_existing"));
	credential.set_field("scopes", json!(REQUIRED_SCOPE.split(' ').collect::<Vec<_>>()));
	credential
}

fn device_id_options(device_id: &'static str) -> Option<LoginOptions> {
	Some(LoginOptions {
		get_device_id: Some(Rc::new(move || device_id.to_string())),
		..LoginOptions::default()
	})
}

#[cfg(feature = "native")]
mod login {
	use super::*;

	const DEVICE_ID: &str = "e61bbe28-07ef-466d-8e5d-a344f94ab305";

	#[test]
	fn registers_a_user_owned_client_and_stores_its_issued_id_and_granted_scopes() {
		let _sequential = common::sequential();
		let authorize_url = Rc::new(RefCell::new(None));
		let exchange_body: RecordedBody = Rc::new(RefCell::new(None));
		let credential = common::run(
			stub_token_endpoint(token_response(REQUIRED_SCOPE), exchange_body.clone(), Rc::default()),
			|| {
				openai_chat_gpt_oauth().login(
					login_interaction(Some("oaiapp_issued"), authorize_url.clone()),
					device_id_options(DEVICE_ID),
				)
			},
		)
		.unwrap();

		let authorize_url = authorize_url.borrow().clone().unwrap();
		let param = |name| search_param(&authorize_url, name);
		assert_eq!(param("client_id").as_deref(), Some("dynamic_agent_client"));
		assert_eq!(param("agent_name_hint").as_deref(), Some("Pi"));
		assert_eq!(param("ext_agent_host_id"), Some(format!("urn:uuid:{DEVICE_ID}")));
		assert_eq!(param("scope").as_deref(), Some(REQUIRED_SCOPE));
		assert_eq!(
			param("redirect_uri").as_deref(),
			Some("http://127.0.0.1:1455/auth/callback")
		);
		assert_eq!(param("resource").as_deref(), Some("https://api.openai.com/v1"));
		assert_eq!(param("code_challenge_method").as_deref(), Some("S256"));
		let exchange_body = exchange_body.borrow().clone().unwrap();
		assert_eq!(exchange_body["client_id"], "oaiapp_issued");
		assert_eq!(exchange_body["code"], "authorization-code");
		assert_eq!(exchange_body["resource"], "https://api.openai.com/v1");
		assert!(!exchange_body["code_verifier"].is_empty());
		assert_eq!(credential.access, "access-token");
		assert_eq!(credential.refresh, "refresh-token");
		assert_eq!(credential.field("clientId"), Some(&json!("oaiapp_issued")));
		assert_eq!(
			credential.field("scopes"),
			Some(&json!(REQUIRED_SCOPE.split(' ').collect::<Vec<_>>()))
		);
	}

	#[test]
	fn uses_the_app_s_agent_name_as_the_name_hint() {
		let _sequential = common::sequential();
		let authorize_url = Rc::new(RefCell::new(None));
		common::run(
			stub_token_endpoint(token_response(REQUIRED_SCOPE), Rc::default(), Rc::default()),
			|| {
				openai_chat_gpt_oauth().login(
					login_interaction(Some("oaiapp_issued"), authorize_url.clone()),
					Some(LoginOptions {
						agent_name: Some("my-app".into()),
						..device_id_options(DEVICE_ID).unwrap()
					}),
				)
			},
		)
		.unwrap();

		let authorize_url = authorize_url.borrow().clone().unwrap();
		assert_eq!(
			search_param(&authorize_url, "agent_name_hint").as_deref(),
			Some("my-app")
		);
	}

	#[test]
	fn rejects_registration_without_an_issued_client_id() {
		let _sequential = common::sequential();
		let calls = Rc::new(RefCell::new(0));
		let error = common::run(
			stub_token_endpoint(token_response(REQUIRED_SCOPE), Rc::default(), calls.clone()),
			|| openai_chat_gpt_oauth().login(login_interaction(None, Rc::default()), device_id_options(DEVICE_ID)),
		)
		.unwrap_err();

		assert!(
			error
				.message
				.contains("registration callback did not contain an issued client ID"),
			"{}",
			error.message
		);
		assert_eq!(*calls.borrow(), 0);
	}

	#[test]
	fn rejects_a_token_response_that_did_not_grant_direct_token_use() {
		let _sequential = common::sequential();
		let error = common::run(
			stub_token_endpoint(
				token_response("openid profile email offline_access resource.invoke"),
				Rc::default(),
				Rc::default(),
			),
			|| {
				openai_chat_gpt_oauth().login(
					login_interaction(Some("oaiapp_issued"), Rc::default()),
					device_id_options(DEVICE_ID),
				)
			},
		)
		.unwrap_err();

		assert!(
			error
				.message
				.contains("grant did not include chatgpt.tokens.use.direct"),
			"{}",
			error.message
		);
	}
}

#[test]
fn requires_a_device_id_before_starting_authorization() {
	let authorize_url = Rc::new(RefCell::new(None));
	let (without_options, invalid_device_id) = common::run(
		stub_token_endpoint(token_response(REQUIRED_SCOPE), Rc::default(), Rc::default()),
		|| async {
			let oauth = openai_chat_gpt_oauth();
			let without_options = (oauth.login)(login_interaction(None, authorize_url.clone()), None).await;
			let invalid_device_id = (oauth.login)(
				login_interaction(None, authorize_url.clone()),
				device_id_options("not-a-uuid"),
			)
			.await;
			(without_options, invalid_device_id)
		},
	);

	assert!(without_options.unwrap_err().message.contains("requires a device ID"));
	assert!(invalid_device_id.unwrap_err().message.contains("requires a device ID"));
	assert!(authorize_url.borrow().is_none());
}

#[test]
fn requires_refresh_responses_to_rotate_the_refresh_token() {
	let mut response = token_response(REQUIRED_SCOPE);
	response.as_object_mut().unwrap().remove("refresh_token");
	let error = common::run(stub_token_endpoint(response, Rc::default(), Rc::default()), || {
		openai_chat_gpt_oauth().refresh(connected_credential(), AbortController::new().signal())
	})
	.unwrap_err();

	assert!(
		error.message.contains("token response has invalid refresh_token"),
		"{}",
		error.message
	);
}

#[test]
fn refreshes_with_the_credential_s_issued_client_id_and_stores_replacement_scopes() {
	let mut response = token_response(REQUIRED_SCOPE);
	response["access_token"] = json!("new-access");
	response["refresh_token"] = json!("new-refresh");
	let refresh_body: RecordedBody = Rc::new(RefCell::new(None));
	let (before, credential, after) = common::run(
		stub_token_endpoint(response, refresh_body.clone(), Rc::default()),
		|| async {
			let before = pi_host::now_ms();
			let credential = openai_chat_gpt_oauth()
				.refresh(connected_credential(), AbortController::new().signal())
				.await;
			(before, credential, pi_host::now_ms())
		},
	);
	let credential = credential.unwrap();

	// expires_in is 3600 seconds; the credential expires 3 minutes early so it is refreshed in time.
	assert!(credential.expires >= before + (3600.0 - 180.0) * 1000.0);
	assert!(credential.expires <= after + (3600.0 - 180.0) * 1000.0);

	let refresh_body = refresh_body.borrow().clone().unwrap();
	assert_eq!(refresh_body["grant_type"], "refresh_token");
	assert_eq!(refresh_body["client_id"], "oaiapp_existing");
	assert_eq!(refresh_body["refresh_token"], "old-refresh");
	assert_eq!(refresh_body["resource"], "https://api.openai.com/v1");
	assert!(!refresh_body.contains_key("scope"));
	assert_eq!(credential.access, "new-access");
	assert_eq!(credential.refresh, "new-refresh");
	assert_eq!(credential.field("clientId"), Some(&json!("oaiapp_existing")));
	assert_eq!(
		credential.field("scopes"),
		Some(&json!(REQUIRED_SCOPE.split(' ').collect::<Vec<_>>()))
	);
}
