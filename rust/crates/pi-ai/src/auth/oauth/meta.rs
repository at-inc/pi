//! Port of `packages/ai/src/auth/oauth/meta.ts`.
//! PORT-STATUS: ported
//!
//! Meta Model API OAuth flow
//!
//! RFC 8628 device authorization grant against https://auth.meta.com (JSON
//! responses). Meta splits identity from API access: the resulting identity
//! token is not accepted for inference, so it is exchanged for a Model API
//! key via the Muse Code key-mint endpoint (minted keys live about a day).
//! The identity token is stored as `refresh` and the minted key as `access`,
//! so the standard OAuth scheduler re-mints the key when it expires with no
//! bespoke renewal machinery. The identity token itself is not renewable
//! (auth.meta.com answers grant_type=refresh_token with 404 and issues no
//! refresh_token), so a 401/403 from mint means the session is dead and the
//! user must sign in again.
//!
//! Port notes: `response.json()` is `response.text()` plus `JSON.parse`. A JSON array body counts as an object, as
//! `typeof [] === "object"` in TS, and has none of the looked-up fields.

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
use crate::utils::text::js_trim;

// Muse Code CLI client id.
const CLIENT_ID: &str = "1031625952748946";
const DEVICE_AUTHORIZATION_URL: &str = "https://auth.meta.com/oidc/device/authorization/";
const DEVICE_TOKEN_URL: &str = "https://auth.meta.com/oidc/device/token/";
const API_KEY_MINT_URL: &str = "https://api.meta.ai/muse-code/key";
const API_KEY_LIFETIME_MS: f64 = 24.0 * 60.0 * 60.0 * 1000.0;
const REQUEST_TIMEOUT_MS: f64 = 30.0 * 1000.0;

#[derive(Clone)]
struct DeviceAuthorization {
	device_code: String,
	user_code: String,
	verification_uri: String,
	interval_seconds: Option<f64>,
	expires_in_seconds: Option<f64>,
}

fn request_signal(signal: &AbortSignal) -> AbortSignal {
	AbortSignal::any(&[AbortSignal::timeout(REQUEST_TIMEOUT_MS), signal.clone()])
}

fn post(url: &str, headers: &[(&str, String)], body: String, signal: &AbortSignal) -> HttpRequest {
	HttpRequest {
		method: "POST".into(),
		url: url.into(),
		headers: headers
			.iter()
			.map(|(name, value)| (name.to_string(), value.clone()))
			.collect::<IndexMap<_, _>>(),
		body: Some(Bytes::from(body)),
		signal: Some(request_signal(signal)),
		..Default::default()
	}
}

fn form_headers() -> [(&'static str, String); 2] {
	[
		("Content-Type", "application/x-www-form-urlencoded".into()),
		("Accept", "application/json".into()),
	]
}

fn form(fields: &[(&str, &str)]) -> String {
	url::form_urlencoded::Serializer::new(String::new())
		.extend_pairs(fields)
		.finish()
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

fn error_detail(json: &Option<Value>) -> String {
	for key in ["error_description", "detail", "message", "error"] {
		if let Some(Value::String(value)) = field(json, key) {
			let value = js_trim(value);
			if !value.is_empty() {
				return format!(": {value}");
			}
		}
	}
	String::new()
}

fn positive_number(value: Option<&Value>) -> Option<f64> {
	value
		.and_then(Value::as_f64)
		.filter(|value| value.is_finite() && *value > 0.0)
}

fn non_empty_string(value: Option<&Value>) -> Option<String> {
	match value {
		Some(Value::String(value)) if !value.is_empty() => Some(value.clone()),
		_ => None,
	}
}

fn trusted(value: Option<&Value>) -> Option<String> {
	trusted_http_url(value.unwrap_or(&Value::Null))
}

async fn start_device_authorization(signal: &AbortSignal) -> PiResult<DeviceAuthorization> {
	let response = pi_host::fetch(
		post(
			DEVICE_AUTHORIZATION_URL,
			&form_headers(),
			form(&[("client_id", CLIENT_ID)]),
			signal,
		),
		None,
	)
	.await?;
	let ok = response.ok();
	let status = response.status;
	let json = read_json(response).await;
	if !ok {
		return Err(PiError::new(format!(
			"Meta device authorization failed with status {status}{}",
			error_detail(&json)
		)));
	}
	let device_code = non_empty_string(field(&json, "device_code"));
	let user_code = non_empty_string(field(&json, "user_code"));
	let verification_uri =
		trusted(field(&json, "verification_uri_complete")).or_else(|| trusted(field(&json, "verification_uri")));
	let (Some(device_code), Some(user_code), Some(verification_uri)) = (device_code, user_code, verification_uri)
	else {
		return Err(PiError::new(format!(
			"Invalid Meta device authorization response: {}",
			pi_host::js::json_stringify(&json)
		)));
	};
	Ok(DeviceAuthorization {
		device_code,
		user_code,
		verification_uri,
		interval_seconds: positive_number(field(&json, "interval")),
		expires_in_seconds: positive_number(field(&json, "expires_in")),
	})
}

async fn poll_for_identity_token(device: DeviceAuthorization, signal: &AbortSignal) -> PiResult<String> {
	let poll_signal = signal.clone();
	let device_code = device.device_code.clone();
	poll_oauth_device_code_flow(OAuthDeviceCodePollOptions {
		interval_seconds: device.interval_seconds,
		expires_in_seconds: device.expires_in_seconds,
		wait_before_first_poll: Some(true),
		signal: signal.clone(),
		poll: Rc::new(move || {
			let signal = poll_signal.clone();
			let device_code = device_code.clone();
			Box::pin(async move {
				let response = pi_host::fetch(
					post(
						DEVICE_TOKEN_URL,
						&form_headers(),
						form(&[
							("grant_type", "urn:ietf:params:oauth:grant-type:device_code"),
							("device_code", &device_code),
							("client_id", CLIENT_ID),
						]),
						&signal,
					),
					None,
				)
				.await?;
				let ok = response.ok();
				let status = response.status;
				let json = read_json(response).await;
				if ok && let Some(access_token) = non_empty_string(field(&json, "access_token")) {
					return Ok(OAuthDeviceCodePollResult::Complete { value: access_token });
				}
				Ok(match field(&json, "error").and_then(Value::as_str) {
					Some("authorization_pending") => OAuthDeviceCodePollResult::Pending,
					Some("slow_down") => OAuthDeviceCodePollResult::SlowDown {
						interval_seconds: positive_number(field(&json, "interval")),
					},
					Some("access_denied") => OAuthDeviceCodePollResult::Failed {
						message: "Meta login was denied.".into(),
					},
					Some("expired_token") => OAuthDeviceCodePollResult::Failed {
						message: "Meta device authorization expired. Please restart login.".into(),
					},
					_ => OAuthDeviceCodePollResult::Failed {
						message: format!(
							"Meta device token request failed with status {status}{}",
							error_detail(&json)
						),
					},
				})
			})
		}),
	})
	.await
}

/// Exchange an identity token for a Model API key. Keys are valid for about a day.
async fn mint_api_key(identity_token: &str, signal: &AbortSignal) -> PiResult<OAuthCredential> {
	let response = pi_host::fetch(
		post(
			API_KEY_MINT_URL,
			&[
				("Accept", "application/json".into()),
				("Authorization", format!("Bearer {identity_token}")),
				("Content-Type", "application/json".into()),
				("x-api-version", "1.0.0".into()),
			],
			"{}".into(),
			signal,
		),
		None,
	)
	.await?;
	let ok = response.ok();
	let status = response.status;
	let json = read_json(response).await;
	if status == 401 || status == 403 {
		// Identity token is not renewable (see file header); only a fresh device flow helps.
		return Err(PiError::new(format!(
			"Meta session expired (status {status}). Run `/login meta` to sign in again.{}",
			error_detail(&json)
		)));
	}
	if !ok {
		return Err(PiError::new(format!(
			"Meta API key mint failed with status {status}{}",
			error_detail(&json)
		)));
	}
	let Some(api_key) = non_empty_string(field(&json, "api_key")) else {
		let action_url = trusted(field(&json, "action_url"));
		return Err(PiError::new(format!(
			"Meta did not issue an API key.{}",
			match action_url {
				Some(action_url) => format!(" Complete setup at {action_url}"),
				None => String::new(),
			}
		)));
	};
	Ok(OAuthCredential::new(
		identity_token,
		api_key,
		pi_host::now_ms() + API_KEY_LIFETIME_MS,
	))
}

async fn login_meta(interaction: ProviderAuthInteraction) -> PiResult<OAuthCredential> {
	let result = async {
		let device = start_device_authorization(&interaction.signal).await?;
		interaction.notify(AuthEvent::DeviceCode {
			user_code: device.user_code.clone(),
			verification_uri: device.verification_uri.clone(),
			open_browser_on_confirm: None,
			copy_code_on_confirm: None,
			interval_seconds: device.interval_seconds,
			expires_in_seconds: device.expires_in_seconds,
		});
		let identity_token = poll_for_identity_token(device, &interaction.signal).await?;
		interaction.notify(AuthEvent::Progress {
			message: "Enabling Meta Model API access...".into(),
		});
		mint_api_key(&identity_token, &interaction.signal).await
	}
	.await;
	// An in-flight fetch rejects with a DOMException on abort; the login UI matches on this message.
	match result {
		Err(_) if interaction.signal.aborted() => Err(PiError::new("Login cancelled")),
		result => result,
	}
}

/// `metaOAuth`.
pub fn meta_oauth() -> OAuthAuth {
	OAuthAuth {
		name: "Meta (Muse subscription)".into(),
		is_subscription: Some(true),
		login_label: Some("Sign in with Meta".into()),

		login: Rc::new(|interaction, _options| Box::pin(login_meta(interaction))),

		refresh: Rc::new(|credential, signal| {
			Box::pin(async move { mint_api_key(&credential.refresh, &signal).await })
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
