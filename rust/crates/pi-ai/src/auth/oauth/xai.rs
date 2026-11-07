//! Port of `packages/ai/src/auth/oauth/xai.ts`.
//! PORT-STATUS: ported
//!
//! xAI OAuth device-code flow.
//!
//! Port notes: `response.json()` is `response.text()` plus `JSON.parse`; a body that fails to read is handled like
//! one that fails to parse. A JSON `null` field is present, as `null !== undefined` in TS. The credential
//! serializes `refresh` before `access` (see `auth/types.rs`).

use std::rc::Rc;

use bytes::Bytes;
use indexmap::IndexMap;
use pi_host::{AbortSignal, HttpRequest, PiError, PiResult};
use serde_json::{Map, Value};

use crate::auth::oauth::device_code::{
	OAuthDeviceCodePollOptions, OAuthDeviceCodePollResult, poll_oauth_device_code_flow,
};
use crate::auth::types::{AuthEvent, ModelAuth, OAuthAuth, OAuthCredential, ProviderAuthInteraction};
use crate::utils::json_parse::json_parse;

const XAI_CLIENT_ID: &str = "b1a00492-073a-47ea-816f-4c329264a828";
const XAI_SCOPE: &str = "openid profile email offline_access grok-cli:access api:access";
const XAI_DEVICE_CODE_URL: &str = "https://auth.x.ai/oauth2/device/code";
const XAI_TOKEN_URL: &str = "https://auth.x.ai/oauth2/token";
// Refresh slightly before the reported expiry to avoid using a token that dies mid-request.
const REFRESH_SKEW_MS: f64 = 5.0 * 60.0 * 1000.0;
const DEFAULT_TOKEN_LIFETIME_SECONDS: f64 = 3600.0;

type JsonObject = Map<String, Value>;

struct OAuthHttpResponse {
	ok: bool,
	status: u16,
	body: JsonObject,
}

#[derive(Clone)]
struct XaiDeviceCode {
	device_code: String,
	user_code: String,
	verification_uri: String,
	verification_uri_complete: Option<String>,
	interval_seconds: Option<f64>,
	expires_in_seconds: f64,
}

fn invalid_field(field: &str) -> PiError {
	PiError::new(format!("Invalid xAI OAuth response field: {field}"))
}

fn required_string(body: &JsonObject, field: &str) -> PiResult<String> {
	match body.get(field) {
		Some(Value::String(value)) if !value.is_empty() => Ok(value.clone()),
		_ => Err(invalid_field(field)),
	}
}

fn positive_number(body: &JsonObject, field: &str) -> PiResult<f64> {
	match body.get(field).and_then(Value::as_f64) {
		Some(value) if value.is_finite() && value > 0.0 => Ok(value),
		_ => Err(invalid_field(field)),
	}
}

// The verification URI is opened in the user's browser; force it to be an https URL
// so a malicious response cannot make `open` launch something else.
fn validate_verification_uri(raw: &str) -> PiResult<String> {
	match url::Url::parse(raw) {
		Ok(url) if url.scheme() == "https" => Ok(url.to_string()),
		_ => Err(PiError::new("Untrusted verification URI in xAI OAuth response")),
	}
}

async fn post_form(url: &str, fields: &[(&str, &str)], signal: &AbortSignal) -> PiResult<OAuthHttpResponse> {
	let mut headers = IndexMap::new();
	headers.insert("Accept".to_string(), "application/json".to_string());
	headers.insert(
		"Content-Type".to_string(),
		"application/x-www-form-urlencoded".to_string(),
	);
	let body = url::form_urlencoded::Serializer::new(String::new())
		.extend_pairs(fields)
		.finish();
	let request = HttpRequest {
		method: "POST".into(),
		url: url.into(),
		headers,
		body: Some(Bytes::from(body)),
		signal: Some(signal.clone()),
	};
	let response = match pi_host::fetch(request, None).await {
		Ok(response) => response,
		Err(error) => {
			if signal.aborted() {
				return Err(PiError::new("Login cancelled"));
			}
			return Err(error);
		}
	};

	let ok = response.ok();
	let status = response.status;
	let parsed = response.text().await.and_then(|text| json_parse(&text));
	let body = match parsed {
		Ok(Value::Object(body)) => body,
		Ok(_) => JsonObject::new(),
		Err(_) => {
			if signal.aborted() {
				return Err(PiError::new("Login cancelled"));
			}
			return Err(PiError::new(format!("xAI OAuth returned invalid JSON (HTTP {status})")));
		}
	};
	Ok(OAuthHttpResponse { ok, status, body })
}

fn request_failure(action: &str, response: &OAuthHttpResponse) -> PiError {
	let text = |key: &str| match response.body.get(key) {
		Some(Value::String(value)) if !value.is_empty() => Some(value.as_str()),
		_ => None,
	};
	let detail = [text("error"), text("error_description")]
		.into_iter()
		.flatten()
		.collect::<Vec<_>>()
		.join(": ");
	PiError::new(format!(
		"xAI OAuth {action} failed (HTTP {}){}",
		response.status,
		if detail.is_empty() {
			String::new()
		} else {
			format!(": {detail}")
		}
	))
}

fn parse_device_code(body: &JsonObject) -> PiResult<XaiDeviceCode> {
	// RFC 8628 allows interval 0 (no minimum wait); fall back to the poller's
	// default instead of failing on non-positive or malformed values.
	let interval_seconds = body
		.get("interval")
		.and_then(Value::as_f64)
		.filter(|interval| interval.is_finite() && *interval > 0.0);
	let verification_uri_complete = match body.get("verification_uri_complete") {
		Some(Value::String(uri)) if !uri.is_empty() => Some(validate_verification_uri(uri)?),
		_ => None,
	};
	Ok(XaiDeviceCode {
		device_code: required_string(body, "device_code")?,
		user_code: required_string(body, "user_code")?,
		verification_uri: validate_verification_uri(&required_string(body, "verification_uri")?)?,
		verification_uri_complete,
		interval_seconds,
		expires_in_seconds: positive_number(body, "expires_in")?,
	})
}

fn credentials_from_token_response(
	body: &JsonObject,
	previous_refresh_token: Option<&str>,
) -> PiResult<OAuthCredential> {
	let access = required_string(body, "access_token")?;
	// xAI may omit refresh_token on refresh when the token is not rotated.
	let refresh = match previous_refresh_token {
		Some(previous) if !body.contains_key("refresh_token") && !previous.is_empty() => previous.to_string(),
		_ => required_string(body, "refresh_token")?,
	};
	let expires_in_seconds = if body.contains_key("expires_in") {
		positive_number(body, "expires_in")?
	} else {
		DEFAULT_TOKEN_LIFETIME_SECONDS
	};
	Ok(OAuthCredential::new(
		refresh,
		access,
		pi_host::now_ms() + expires_in_seconds * 1000.0 - REFRESH_SKEW_MS,
	))
}

async fn request_device_code(signal: &AbortSignal) -> PiResult<XaiDeviceCode> {
	let response = post_form(
		XAI_DEVICE_CODE_URL,
		&[("client_id", XAI_CLIENT_ID), ("scope", XAI_SCOPE), ("referrer", "pi")],
		signal,
	)
	.await?;
	if !response.ok {
		return Err(request_failure("device authorization", &response));
	}
	parse_device_code(&response.body)
}

async fn poll_for_tokens(device: XaiDeviceCode, signal: &AbortSignal) -> PiResult<OAuthCredential> {
	let poll_signal = signal.clone();
	let device_code = device.device_code.clone();
	poll_oauth_device_code_flow(OAuthDeviceCodePollOptions {
		interval_seconds: device.interval_seconds,
		expires_in_seconds: Some(device.expires_in_seconds),
		wait_before_first_poll: Some(true),
		signal: signal.clone(),
		poll: Rc::new(move || {
			let signal = poll_signal.clone();
			let device_code = device_code.clone();
			Box::pin(async move {
				let response = post_form(
					XAI_TOKEN_URL,
					&[
						("grant_type", "urn:ietf:params:oauth:grant-type:device_code"),
						("client_id", XAI_CLIENT_ID),
						("device_code", &device_code),
					],
					&signal,
				)
				.await?;

				if response.ok {
					return Ok(OAuthDeviceCodePollResult::Complete {
						value: credentials_from_token_response(&response.body, None)?,
					});
				}

				let error = response.body.get("error").and_then(Value::as_str);
				Ok(match error {
					Some("authorization_pending") => OAuthDeviceCodePollResult::Pending,
					Some("slow_down") => OAuthDeviceCodePollResult::SlowDown {
						interval_seconds: response.body.get("interval").and_then(Value::as_f64),
					},
					Some("access_denied" | "authorization_denied") => OAuthDeviceCodePollResult::Failed {
						message: "xAI device authorization was denied".into(),
					},
					Some("expired_token") => OAuthDeviceCodePollResult::Failed {
						message: "xAI device code expired".into(),
					},
					_ => OAuthDeviceCodePollResult::Failed {
						message: request_failure("device token polling", &response).message,
					},
				})
			})
		}),
	})
	.await
}

async fn login_xai(interaction: ProviderAuthInteraction) -> PiResult<OAuthCredential> {
	let device = request_device_code(&interaction.signal).await?;
	interaction.notify(AuthEvent::DeviceCode {
		user_code: device.user_code.clone(),
		verification_uri: device
			.verification_uri_complete
			.clone()
			.unwrap_or_else(|| device.verification_uri.clone()),
		open_browser_on_confirm: None,
		copy_code_on_confirm: None,
		interval_seconds: device.interval_seconds,
		expires_in_seconds: Some(device.expires_in_seconds),
	});
	poll_for_tokens(device, &interaction.signal).await
}

async fn refresh_xai_token(refresh_token: &str, signal: &AbortSignal) -> PiResult<OAuthCredential> {
	let response = post_form(
		XAI_TOKEN_URL,
		&[
			("grant_type", "refresh_token"),
			("client_id", XAI_CLIENT_ID),
			("refresh_token", refresh_token),
		],
		signal,
	)
	.await?;
	if !response.ok {
		return Err(request_failure("token refresh", &response));
	}
	credentials_from_token_response(&response.body, Some(refresh_token))
}

/// `xaiOAuth`.
pub fn xai_oauth() -> OAuthAuth {
	OAuthAuth {
		name: "xAI (Grok/X subscription)".into(),
		is_subscription: Some(true),
		login_label: Some("Sign in with SuperGrok or X Premium".into()),
		login: Rc::new(|interaction, _options| Box::pin(login_xai(interaction))),
		refresh: Rc::new(|credential, signal| {
			Box::pin(async move { refresh_xai_token(&credential.refresh, &signal).await })
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
