//! Port of `packages/ai/src/auth/oauth/radius.ts`.
//! PORT-STATUS: ported
//!
//! Radius gateway OAuth flow.
//!
//! Radius is a pi-messages gateway. OAuth client APIs live on the configured
//! gateway; only the interactive browser authorization endpoint is discovered.
//! Model catalog loading is owned by the Radius provider.
//!
//! NOTE: This module uses node:http (via callback-server.ts) for the OAuth callback server.
//! It is only intended for CLI use, not browser environments.
//!
//! Port notes: `OAuthResponseError` is a `PiError` named `OAuthResponseError` with `status` and `data.oauthError`.
//! `response.json()` is `response.text()` plus `JSON.parse`. Token fields of a token response that are not strings
//! become `""` where TS keeps `undefined` (which `JSON.stringify` drops). The credential serializes
//! `refresh, access, expires` before `scope` (see `auth/types.rs`). A device authorization `device_code` or `user_code`
//! that is truthy but not a string counts as missing. The browser login needs the `native` feature
//! for its callback server (`auth/oauth/callback_server.rs`).

use std::rc::Rc;

use bytes::Bytes;
use indexmap::IndexMap;
use pi_host::{AbortSignal, HttpRequest, HttpResponse, PiError, PiResult};
use serde_json::{Value, json};

use crate::auth::oauth::callback_server::{OAuthCallbackServerOptions, start_oauth_callback_server};
use crate::auth::oauth::device_code::{
	OAuthDeviceCodePollOptions, OAuthDeviceCodePollResult, poll_oauth_device_code_flow,
};
use crate::auth::oauth::openrouter::random_uuid;
use crate::auth::oauth::pkce::generate_pkce;
use crate::auth::oauth::trusted_url::trusted_http_url;
use crate::auth::types::{
	AuthEvent, AuthPrompt, AuthPromptKind, AuthSelectOption, ModelAuth, OAuthAuth, OAuthCredential,
	ProviderAuthInteraction,
};
use crate::providers::radius_config::normalize_radius_gateway_url;
use crate::utils::json_parse::json_parse;

const CALLBACK_HOST: &str = "127.0.0.1";
const CALLBACK_PORT: u16 = 1456;
const CALLBACK_PATH: &str = "/oauth/callback";
const REDIRECT_URI: &str = "http://127.0.0.1:1456/oauth/callback";
const TOKEN_EXPIRY_SKEW_MS: f64 = 60_000.0;
const LOGIN_METHOD_BROWSER: &str = "browser";
const LOGIN_METHOD_DEVICE_CODE: &str = "device-code";
const OAUTH_CLIENT_ID: &str = "pi-gateway";
const OAUTH_SCOPE: &str = "gateway offline_access";
const OAUTH_DEVICE_CODE_GRANT_TYPE: &str = "urn:ietf:params:oauth:grant-type:device_code";
const OAUTH_RESPONSE_ERROR: &str = "OAuthResponseError";

#[derive(Clone, Debug)]
struct DeviceAuthorizationResponse {
	device_code: String,
	user_code: String,
	verification_uri: String,
	expires_in: f64,
	interval: Option<f64>,
}

/// `new URL(path, gateway)`.
fn gateway_url(path: &str, gateway: &str) -> PiResult<String> {
	url::Url::parse(gateway)
		.and_then(|base| base.join(path))
		.map(|url| url.to_string())
		.map_err(|_| PiError::type_error("Invalid URL"))
}

fn form_headers() -> IndexMap<String, String> {
	IndexMap::from([
		("accept".to_string(), "application/json".to_string()),
		(
			"content-type".to_string(),
			"application/x-www-form-urlencoded".to_string(),
		),
	])
}

fn form_body(pairs: &[(&str, &str)]) -> Bytes {
	Bytes::from(
		url::form_urlencoded::Serializer::new(String::new())
			.extend_pairs(pairs)
			.finish(),
	)
}

async fn load_radius_oauth_discovery(gateway: &str, signal: &AbortSignal) -> PiResult<String> {
	let response = pi_host::fetch(
		HttpRequest {
			method: "GET".into(),
			url: gateway_url("/v1/oauth", gateway)?,
			headers: IndexMap::from([("accept".to_string(), "application/json".to_string())]),
			body: None,
			signal: Some(signal.clone()),
		},
		None,
	)
	.await?;

	if !response.ok() {
		let status = response.status;
		let text = response.text().await?;
		return Err(PiError::new(format!(
			"Could not load Radius OAuth config from {gateway}: {status} {text}"
		)));
	}

	let discovery = json_parse(&response.text().await?)?;
	null_property(&discovery, "authorizationEndpoint")?;
	let Some(authorization_endpoint) = discovery.get("authorizationEndpoint").and_then(trusted_http_url) else {
		return Err(PiError::new(format!("Invalid Radius OAuth config from {gateway}")));
	};
	Ok(authorization_endpoint)
}

fn oauth_response_error(
	status: u16,
	oauth_error: Option<String>,
	description: Option<String>,
	message: &str,
) -> PiError {
	let detail = match (&oauth_error, description) {
		(Some(oauth_error), Some(description)) if !oauth_error.is_empty() && !description.is_empty() => {
			format!("{oauth_error}: {description}")
		}
		(Some(oauth_error), _) if !oauth_error.is_empty() => oauth_error.clone(),
		(_, Some(description)) if !description.is_empty() => description,
		_ => status.to_string(),
	};
	let mut error = PiError::named(OAUTH_RESPONSE_ERROR, format!("{message}: {detail}"));
	error.status = Some(status);
	error.data = Some(json!({ "oauthError": oauth_error }));
	error
}

/// `error.oauthError` of an `OAuthResponseError`; `None` for any other error.
fn oauth_error_code(error: &PiError) -> Option<Option<&str>> {
	if error.name != OAUTH_RESPONSE_ERROR {
		return None;
	}
	Some(
		error
			.data
			.as_ref()
			.and_then(|data| data.get("oauthError"))
			.and_then(Value::as_str),
	)
}

async fn read_oauth_response_error(response: HttpResponse, message: &str) -> PiError {
	let status = response.status;
	let text = response.text().await.unwrap_or_default();
	let mut oauth_error = None;
	let mut description = None;

	if !text.is_empty() {
		// Reading a property of a parsed `null` throws inside the TS `try`, so the text becomes the description.
		match json_parse(&text).ok().filter(|data| !data.is_null()) {
			Some(data) => {
				oauth_error = data.get("error").and_then(Value::as_str).map(str::to_string);
				description = data
					.get("error_description")
					.and_then(Value::as_str)
					.map(str::to_string);
			}
			None => description = Some(text),
		}
	}

	oauth_response_error(status, oauth_error, description, message)
}

/// Reading `key` of a parsed `null` throws like V8.
fn null_property(value: &Value, key: &str) -> PiResult<()> {
	if value.is_null() {
		return Err(PiError::type_error(format!(
			"Cannot read properties of null (reading '{key}')"
		)));
	}
	Ok(())
}

/// `fetch` of a form POST; a failure after the signal aborted is `Login cancelled`.
async fn post_form(url: String, body: Bytes, signal: &AbortSignal) -> PiResult<HttpResponse> {
	pi_host::fetch(
		HttpRequest {
			method: "POST".into(),
			url,
			headers: form_headers(),
			body: Some(body),
			signal: Some(signal.clone()),
		},
		None,
	)
	.await
	.map_err(|error| {
		if signal.aborted() {
			PiError::new("Login cancelled")
		} else {
			error
		}
	})
}

/// JS `Number(value)` arithmetic on a JSON field: `undefined` is NaN.
fn js_number(value: Option<&Value>) -> f64 {
	match value {
		None => f64::NAN,
		Some(Value::Null) => 0.0,
		Some(Value::Bool(value)) => f64::from(u8::from(*value)),
		Some(Value::Number(number)) => number.as_f64().unwrap_or(f64::NAN),
		Some(Value::String(value)) => pi_host::js::string_to_number(value),
		Some(_) => f64::NAN,
	}
}

fn string_field(value: Option<&Value>) -> String {
	value.and_then(Value::as_str).unwrap_or_default().to_string()
}

async fn request_oauth_token(gateway: &str, body: Bytes, signal: &AbortSignal) -> PiResult<OAuthCredential> {
	let response = post_form(gateway_url("/v1/oauth/token", gateway)?, body, signal).await?;

	if !response.ok() {
		return Err(read_oauth_response_error(response, "Radius OAuth token request failed").await);
	}

	let data = json_parse(&response.text().await?)?;
	null_property(&data, "access_token")?;

	let mut credential = OAuthCredential::new(
		string_field(data.get("refresh_token")),
		string_field(data.get("access_token")),
		pi_host::now_ms() + js_number(data.get("expires_in")) * 1000.0 - TOKEN_EXPIRY_SKEW_MS,
	);
	if let Some(scope) = data.get("scope") {
		credential.set_field("scope", scope.clone());
	}
	Ok(credential)
}

async fn login_with_browser(
	gateway: String,
	authorization_endpoint: &str,
	interaction: &ProviderAuthInteraction,
) -> PiResult<OAuthCredential> {
	let pkce = generate_pkce().await;
	let state = random_uuid();
	let mut authorize_url = url::Url::parse(authorization_endpoint).map_err(|_| PiError::type_error("Invalid URL"))?;
	authorize_url.set_query(Some(
		&url::form_urlencoded::Serializer::new(String::new())
			.append_pair("response_type", "code")
			.append_pair("client_id", OAUTH_CLIENT_ID)
			.append_pair("redirect_uri", REDIRECT_URI)
			.append_pair("scope", OAUTH_SCOPE)
			.append_pair("code_challenge", &pkce.challenge)
			.append_pair("code_challenge_method", "S256")
			.append_pair("handoff", "url")
			.append_pair("state", &state)
			.finish(),
	));

	let verifier = pkce.verifier;
	let signal = interaction.signal.clone();
	let callback = start_oauth_callback_server(OAuthCallbackServerOptions {
		provider_name: "Radius".into(),
		host: CALLBACK_HOST.into(),
		port: CALLBACK_PORT,
		path: CALLBACK_PATH.into(),
		redirect_host: None,
		state: Some(state),
		complete: Rc::new(move |code| {
			let gateway = gateway.clone();
			let verifier = verifier.clone();
			let signal = signal.clone();
			Box::pin(async move {
				request_oauth_token(
					&gateway,
					form_body(&[
						("grant_type", "authorization_code"),
						("client_id", OAUTH_CLIENT_ID),
						("redirect_uri", REDIRECT_URI),
						("code", &code),
						("code_verifier", &verifier),
					]),
					&signal,
				)
				.await
			})
		}),
		signal: Some(interaction.signal.clone()),
		timeout_ms: None,
	})
	.await?;
	interaction.notify(AuthEvent::Progress {
		message: format!("Listening for OAuth callback on {REDIRECT_URI}"),
	});
	interaction.notify(AuthEvent::AuthUrl {
		url: authorize_url.to_string(),
		instructions: Some("Continue in your browser.".into()),
	});

	let result = callback.wait().await;
	callback.close();
	result?.ok_or_else(|| PiError::new("OAuth callback did not complete."))
}

async fn request_device_authorization(gateway: &str, signal: &AbortSignal) -> PiResult<DeviceAuthorizationResponse> {
	let response = post_form(
		gateway_url("/v1/oauth/device", gateway)?,
		form_body(&[("client_id", OAUTH_CLIENT_ID), ("scope", OAUTH_SCOPE)]),
		signal,
	)
	.await?;

	if !response.ok() {
		return Err(read_oauth_response_error(response, "Radius OAuth device authorization failed").await);
	}

	let data = json_parse(&response.text().await?)?;
	null_property(&data, "device_code")?;
	let truthy_string = |key: &str| match data.get(key) {
		Some(Value::String(value)) if !value.is_empty() => Some(value.clone()),
		_ => None,
	};
	let expires_in = data
		.get("expires_in")
		.filter(|value| is_truthy(value))
		.map(|value| js_number(Some(value)));
	let (Some(device_code), Some(user_code), Some(verification_uri), Some(expires_in)) = (
		truthy_string("device_code"),
		truthy_string("user_code"),
		data.get("verification_uri").filter(|value| is_truthy(value)),
		expires_in,
	) else {
		return Err(PiError::new(
			"Radius OAuth device authorization response is missing required fields",
		));
	};
	let Some(verification_uri) = trusted_http_url(verification_uri) else {
		return Err(PiError::new(
			"Untrusted verification_uri in Radius OAuth device authorization response",
		));
	};

	Ok(DeviceAuthorizationResponse {
		device_code,
		user_code,
		verification_uri,
		expires_in,
		interval: data.get("interval").and_then(Value::as_f64),
	})
}

/// JS truthiness of a JSON value.
fn is_truthy(value: &Value) -> bool {
	match value {
		Value::Null => false,
		Value::Bool(value) => *value,
		Value::Number(value) => value.as_f64().is_some_and(|value| value != 0.0 && !value.is_nan()),
		Value::String(value) => !value.is_empty(),
		Value::Array(_) | Value::Object(_) => true,
	}
}

async fn login_with_device_code(gateway: String, interaction: &ProviderAuthInteraction) -> PiResult<OAuthCredential> {
	let device = request_device_authorization(&gateway, &interaction.signal).await?;
	interaction.notify(AuthEvent::DeviceCode {
		user_code: device.user_code.clone(),
		verification_uri: device.verification_uri.clone(),
		open_browser_on_confirm: None,
		copy_code_on_confirm: None,
		interval_seconds: device.interval,
		expires_in_seconds: Some(device.expires_in),
	});

	let signal = interaction.signal.clone();
	let device_code = device.device_code.clone();
	poll_oauth_device_code_flow(OAuthDeviceCodePollOptions {
		interval_seconds: device.interval,
		expires_in_seconds: Some(device.expires_in),
		wait_before_first_poll: None,
		signal: interaction.signal.clone(),
		poll: Rc::new(move || {
			let gateway = gateway.clone();
			let device_code = device_code.clone();
			let signal = signal.clone();
			Box::pin(async move {
				let result = request_oauth_token(
					&gateway,
					form_body(&[
						("grant_type", OAUTH_DEVICE_CODE_GRANT_TYPE),
						("client_id", OAUTH_CLIENT_ID),
						("device_code", &device_code),
					]),
					&signal,
				)
				.await;
				let error = match result {
					Ok(credentials) => return Ok(OAuthDeviceCodePollResult::Complete { value: credentials }),
					Err(error) => error,
				};
				let Some(oauth_error) = oauth_error_code(&error) else {
					return Err(error);
				};
				match oauth_error {
					Some("authorization_pending") => Ok(OAuthDeviceCodePollResult::Pending),
					Some("slow_down") => Ok(OAuthDeviceCodePollResult::SlowDown { interval_seconds: None }),
					Some("expired_token") => Ok(OAuthDeviceCodePollResult::Failed {
						message: "Device authorization expired.".into(),
					}),
					Some("access_denied") => Ok(OAuthDeviceCodePollResult::Failed {
						message: "Device authorization was denied.".into(),
					}),
					_ => Err(error),
				}
			})
		}),
	})
	.await
}

/// `RadiusOAuthOptions`.
#[derive(Clone, Debug)]
pub struct RadiusOAuthOptions {
	pub name: String,
	pub gateway: String,
}

async fn login_radius(
	name: String,
	gateway: String,
	interaction: ProviderAuthInteraction,
) -> PiResult<OAuthCredential> {
	let login_method = interaction
		.prompt(AuthPrompt::new(AuthPromptKind::Select {
			message: format!("Sign in to {name}:"),
			options: vec![
				AuthSelectOption::new(LOGIN_METHOD_BROWSER, "Sign in with browser (recommended)"),
				AuthSelectOption::new(
					LOGIN_METHOD_DEVICE_CODE,
					"Sign in with device code (when signing in from another device)",
				),
			],
		}))
		.await?;

	if login_method == LOGIN_METHOD_DEVICE_CODE {
		return login_with_device_code(gateway, &interaction).await;
	}
	if login_method == LOGIN_METHOD_BROWSER {
		let authorization_endpoint = load_radius_oauth_discovery(&gateway, &interaction.signal).await?;
		return login_with_browser(gateway, &authorization_endpoint, &interaction).await;
	}
	Err(PiError::new(format!("Unknown {name} sign-in method: {login_method}")))
}

/// `createRadiusOAuth`.
pub fn create_radius_oauth(options: RadiusOAuthOptions) -> OAuthAuth {
	let gateway = normalize_radius_gateway_url(&options.gateway);
	let name = options.name.clone();
	let refresh_gateway = gateway.clone();

	OAuthAuth {
		name: options.name,
		is_subscription: None,
		login_label: None,
		login: Rc::new(move |interaction, _options| Box::pin(login_radius(name.clone(), gateway.clone(), interaction))),
		refresh: Rc::new(move |credential, signal| {
			let gateway = refresh_gateway.clone();
			Box::pin(async move {
				request_oauth_token(
					&gateway,
					form_body(&[
						("grant_type", "refresh_token"),
						("client_id", OAUTH_CLIENT_ID),
						("refresh_token", &credential.refresh),
					]),
					&signal,
				)
				.await
			})
		}),
		to_auth: Rc::new(|credential| {
			Box::pin(async move {
				Ok(ModelAuth {
					api_key: Some(credential.access),
					..ModelAuth::default()
				})
			})
		}),
	}
}
