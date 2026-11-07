//! Port of `packages/ai/src/auth/oauth/kimi-coding.ts`.
//! PORT-STATUS: ported
//!
//! Kimi Code (subscription) OAuth flow
//!
//! RFC 8628 device authorization for the selected mainland or international
//! deployment. The credential keeps both endpoints for refresh and inference.
//!
//! Port notes: `response.json()` is `response.text()` plus `JSON.parse`; a JSON array body counts as an object, as
//! `typeof [] === "object"` in TS. The credential serializes `refresh, access, expires` before the endpoint fields
//! (see `auth/types.rs`).

use std::rc::Rc;

use bytes::Bytes;
use indexmap::IndexMap;
use pi_host::{AbortSignal, HttpRequest, HttpResponse, PiError, PiResult};
use serde_json::Value;

use crate::auth::oauth::device_code::{
	OAuthDeviceCodePollOptions, OAuthDeviceCodePollResult, poll_oauth_device_code_flow,
};
use crate::auth::oauth::trusted_url::trusted_http_url;
use crate::auth::types::{AuthEvent, ModelAuth, OAuthAuth, OAuthCredential, ProviderAuthInteraction};
use crate::utils::json_parse::json_parse;
use crate::utils::kimi_coding::{
	KimiCodingEndpoints, KimiCodingEndpointsInput, kimi_coding_endpoints, select_kimi_coding_endpoints,
};
use crate::utils::provider_env::get_provider_env_value;
use crate::utils::sleep::sleep;

const CLIENT_ID: &str = "17e5f671-d194-4dfb-9706-5516cb48c098";
const DEVICE_CODE_TIMEOUT_SECONDS: f64 = 15.0 * 60.0;
const DEFAULT_POLL_INTERVAL_SECONDS: f64 = 5.0;
const REQUEST_TIMEOUT_MS: f64 = 30.0 * 1000.0;
const REFRESH_MAX_RETRIES: u32 = 3;

#[derive(Clone)]
struct DeviceAuthorization {
	device_code: String,
	user_code: String,
	verification_uri_complete: String,
	interval_seconds: f64,
	expires_in_seconds: f64,
}

struct TokenResponse {
	access: String,
	refresh: String,
	expires: f64,
}

fn string_field(credential: &OAuthCredential, key: &str) -> Option<String> {
	match credential.field(key) {
		Some(Value::String(value)) => Some(value.clone()),
		_ => None,
	}
}

fn credential_endpoints(credential: &OAuthCredential) -> PiResult<KimiCodingEndpoints> {
	// Pin new logins to their deployment; older credentials still honor the existing host override.
	let oauth_host = string_field(credential, "oauthHost").or_else(|| {
		get_provider_env_value("KIMI_CODE_OAUTH_HOST", None).or_else(|| get_provider_env_value("KIMI_OAUTH_HOST", None))
	});
	let base_url = string_field(credential, "baseUrl").or_else(|| get_provider_env_value("KIMI_CODE_BASE_URL", None));
	kimi_coding_endpoints(KimiCodingEndpointsInput { oauth_host, base_url })
}

fn request_signal(signal: &AbortSignal) -> AbortSignal {
	AbortSignal::any(&[AbortSignal::timeout(REQUEST_TIMEOUT_MS), signal.clone()])
}

fn form_request(url: String, fields: &[(&str, &str)], signal: &AbortSignal) -> HttpRequest {
	let mut headers = IndexMap::new();
	headers.insert(
		"Content-Type".to_string(),
		"application/x-www-form-urlencoded".to_string(),
	);
	headers.insert("Accept".to_string(), "application/json".to_string());
	let body = url::form_urlencoded::Serializer::new(String::new())
		.extend_pairs(fields)
		.finish();
	HttpRequest {
		method: "POST".into(),
		url,
		headers,
		body: Some(Bytes::from(body)),
		signal: Some(request_signal(signal)),
	}
}

/// The JSON object (or array) body, or `null`.
async fn read_json(response: HttpResponse) -> Option<Value> {
	let text = response.text().await.ok()?;
	json_parse(&text)
		.ok()
		.filter(|json| json.is_object() || json.is_array())
}

/// `json?.[key]`.
fn field<'a>(json: &'a Option<Value>, key: &str) -> Option<&'a Value> {
	json.as_ref()
		.and_then(Value::as_object)
		.and_then(|object| object.get(key))
}

/// `": " + text` for a non-empty text, else nothing.
fn with_detail(text: &str) -> String {
	if text.is_empty() {
		String::new()
	} else {
		format!(": {text}")
	}
}

fn positive_number_or(value: Option<&Value>, fallback: f64) -> f64 {
	value
		.and_then(Value::as_f64)
		.filter(|value| value.is_finite() && *value > 0.0)
		.unwrap_or(fallback)
}

async fn start_device_authorization(oauth_host: &str, signal: &AbortSignal) -> PiResult<DeviceAuthorization> {
	let response = pi_host::fetch(
		form_request(
			format!("{oauth_host}/api/oauth/device_authorization"),
			&[("client_id", CLIENT_ID)],
			signal,
		),
		None,
	)
	.await?;

	if !response.ok() {
		let status = response.status;
		let text = response.text().await.unwrap_or_default();
		return Err(PiError::new(format!(
			"Kimi Code device authorization failed with status {status}{}",
			with_detail(&text)
		)));
	}

	let json = read_json(response).await;
	let device_code = field(&json, "device_code").and_then(Value::as_str);
	let user_code = field(&json, "user_code").and_then(Value::as_str);
	let verification_uri = trusted_http_url(field(&json, "verification_uri").unwrap_or(&Value::Null));
	let verification_uri_complete = trusted_http_url(field(&json, "verification_uri_complete").unwrap_or(&Value::Null));
	let (Some(device_code), Some(user_code), Some(_), Some(verification_uri_complete)) =
		(device_code, user_code, verification_uri, verification_uri_complete)
	else {
		return Err(PiError::new(format!(
			"Invalid Kimi Code device authorization response: {}",
			pi_host::js::json_stringify(&json)
		)));
	};

	Ok(DeviceAuthorization {
		device_code: device_code.to_string(),
		user_code: user_code.to_string(),
		verification_uri_complete,
		interval_seconds: positive_number_or(field(&json, "interval"), DEFAULT_POLL_INTERVAL_SECONDS),
		expires_in_seconds: positive_number_or(field(&json, "expires_in"), DEVICE_CODE_TIMEOUT_SECONDS),
	})
}

fn parse_token_response(json: &Option<Value>, operation: &str) -> PiResult<TokenResponse> {
	let access_token = field(json, "access_token").and_then(Value::as_str);
	let refresh_token = field(json, "refresh_token").and_then(Value::as_str);
	let expires_in = field(json, "expires_in").and_then(Value::as_f64);
	match (access_token, refresh_token, expires_in) {
		(Some(access), Some(refresh), Some(expires_in))
			if !access.is_empty() && !refresh.is_empty() && expires_in.is_finite() && expires_in > 0.0 =>
		{
			Ok(TokenResponse {
				access: access.to_string(),
				refresh: refresh.to_string(),
				expires: pi_host::now_ms() + expires_in * 1000.0,
			})
		}
		_ => Err(PiError::new(format!(
			"Kimi Code token {operation} response missing fields: {}",
			pi_host::js::json_stringify(json)
		))),
	}
}

async fn poll_for_token(
	oauth_host: &str,
	device: DeviceAuthorization,
	signal: &AbortSignal,
) -> PiResult<TokenResponse> {
	let poll_signal = signal.clone();
	let token_url = format!("{oauth_host}/api/oauth/token");
	let device_code = device.device_code.clone();
	poll_oauth_device_code_flow(OAuthDeviceCodePollOptions {
		interval_seconds: Some(device.interval_seconds),
		expires_in_seconds: Some(device.expires_in_seconds),
		wait_before_first_poll: Some(true),
		signal: signal.clone(),
		poll: Rc::new(move || {
			let signal = poll_signal.clone();
			let token_url = token_url.clone();
			let device_code = device_code.clone();
			Box::pin(async move {
				let response = pi_host::fetch(
					form_request(
						token_url,
						&[
							("client_id", CLIENT_ID),
							("device_code", &device_code),
							("grant_type", "urn:ietf:params:oauth:grant-type:device_code"),
						],
						&signal,
					),
					None,
				)
				.await?;

				let status = response.status;
				if status >= 500 {
					let text = response.text().await.unwrap_or_default();
					return Ok(OAuthDeviceCodePollResult::Failed {
						message: format!(
							"Kimi Code device token request failed with status {status}{}",
							with_detail(&text)
						),
					});
				}

				let ok = response.ok();
				let json = read_json(response).await;
				if ok && field(&json, "access_token").is_some_and(Value::is_string) {
					return Ok(match parse_token_response(&json, "poll") {
						Ok(value) => OAuthDeviceCodePollResult::Complete { value },
						Err(error) => OAuthDeviceCodePollResult::Failed { message: error.message },
					});
				}

				let error = field(&json, "error");
				let description = match field(&json, "error_description") {
					Some(Value::String(description)) => format!(": {description}"),
					_ => String::new(),
				};
				Ok(match error.and_then(Value::as_str) {
					Some("authorization_pending") => OAuthDeviceCodePollResult::Pending,
					Some("slow_down") => OAuthDeviceCodePollResult::SlowDown {
						interval_seconds: field(&json, "interval")
							.and_then(Value::as_f64)
							.filter(|interval| *interval > 0.0),
					},
					Some("expired_token") => OAuthDeviceCodePollResult::Failed {
						message: "Kimi Code device authorization expired. Please restart login.".into(),
					},
					Some("access_denied") => OAuthDeviceCodePollResult::Failed {
						message: "Kimi Code login was denied.".into(),
					},
					error => OAuthDeviceCodePollResult::Failed {
						message: format!(
							"Kimi Code device token request failed (status {status}){}",
							match error {
								Some(error) => format!(": {error}{description}"),
								None => String::new(),
							}
						),
					},
				})
			})
		}),
	})
	.await
}

fn is_retryable_refresh_failure(status: u16) -> bool {
	status == 429 || status >= 500
}

async fn refresh_token(oauth_host: &str, refresh_token_value: &str, signal: &AbortSignal) -> PiResult<TokenResponse> {
	let mut last_error: Option<PiError> = None;
	for attempt in 0..=REFRESH_MAX_RETRIES {
		if attempt > 0 {
			sleep(1000.0 * f64::from(2u32.pow(attempt - 1)), signal).await?;
		}
		if signal.aborted() {
			return Err(PiError::new("Kimi Code token refresh aborted"));
		}

		let response = match pi_host::fetch(
			form_request(
				format!("{oauth_host}/api/oauth/token"),
				&[
					("client_id", CLIENT_ID),
					("grant_type", "refresh_token"),
					("refresh_token", refresh_token_value),
				],
				signal,
			),
			None,
		)
		.await
		{
			Ok(response) => response,
			Err(error) => {
				last_error = Some(error);
				continue;
			}
		};

		let ok = response.ok();
		let status = response.status;
		let json = read_json(response).await;
		if ok {
			return parse_token_response(&json, "refresh");
		}

		// Unauthorized: the stored credential is dead; Models clears it and prompts re-login.
		if status == 401 || status == 403 || field(&json, "error").and_then(Value::as_str) == Some("invalid_grant") {
			let description = match field(&json, "error_description") {
				Some(Value::String(description)) => format!(": {description}"),
				_ => String::new(),
			};
			return Err(PiError::new(format!(
				"Kimi Code token refresh unauthorized (status {status}){description}"
			)));
		}

		if is_retryable_refresh_failure(status) && attempt < REFRESH_MAX_RETRIES {
			last_error = Some(PiError::new(format!(
				"Kimi Code token refresh failed with status {status}"
			)));
			continue;
		}

		let text = pi_host::js::json_stringify(&json);
		return Err(PiError::new(format!(
			"Kimi Code token refresh failed with status {status}{}",
			with_detail(&text)
		)));
	}

	Err(last_error.unwrap_or_else(|| PiError::new("Kimi Code token refresh failed")))
}

/// `{ ...credential, ...token, ...endpoints }`.
fn with_token_and_endpoints(
	mut credential: OAuthCredential,
	token: TokenResponse,
	endpoints: KimiCodingEndpoints,
) -> OAuthCredential {
	credential.access = token.access;
	credential.refresh = token.refresh;
	credential.expires = token.expires;
	credential.set_field("oauthHost", Value::String(endpoints.oauth_host));
	credential.set_field("baseUrl", Value::String(endpoints.base_url));
	credential
}

async fn login_kimi_coding(interaction: ProviderAuthInteraction) -> PiResult<OAuthCredential> {
	let endpoints = select_kimi_coding_endpoints(&interaction).await?;
	let device = start_device_authorization(&endpoints.oauth_host, &interaction.signal).await?;
	interaction.notify(AuthEvent::DeviceCode {
		user_code: device.user_code.clone(),
		verification_uri: device.verification_uri_complete.clone(),
		open_browser_on_confirm: None,
		copy_code_on_confirm: None,
		interval_seconds: Some(device.interval_seconds),
		expires_in_seconds: Some(device.expires_in_seconds),
	});
	let token = poll_for_token(&endpoints.oauth_host, device, &interaction.signal).await?;
	Ok(with_token_and_endpoints(OAuthCredential::default(), token, endpoints))
}

/// `kimiCodingOAuth`.
pub fn kimi_coding_oauth() -> OAuthAuth {
	OAuthAuth {
		name: "Kimi Code (subscription)".into(),
		is_subscription: Some(true),
		login_label: Some("Sign in with Kimi Code".into()),

		login: Rc::new(|interaction, _options| Box::pin(login_kimi_coding(interaction))),

		refresh: Rc::new(|credential, signal| {
			Box::pin(async move {
				let endpoints = credential_endpoints(&credential)?;
				let token = refresh_token(&endpoints.oauth_host, &credential.refresh, &signal).await?;
				Ok(with_token_and_endpoints(credential, token, endpoints))
			})
		}),

		to_auth: Rc::new(|credential| {
			Box::pin(async move {
				let base_url = credential_endpoints(&credential)?.base_url;
				let mut headers = IndexMap::new();
				headers.insert(
					"Authorization".to_string(),
					Some(format!("Bearer {}", credential.access)),
				);
				Ok(ModelAuth {
					api_key: None,
					headers: Some(headers),
					base_url: Some(base_url),
				})
			})
		}),
	}
}
