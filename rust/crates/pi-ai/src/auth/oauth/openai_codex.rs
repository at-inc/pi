//! Port of `packages/ai/src/auth/oauth/openai-codex.ts`.
//! PORT-STATUS: reviewed
//!
//! OpenAI Codex (ChatGPT OAuth) flow
//!
//! NOTE: This module uses Node.js crypto and http (via callback-server.ts) for the OAuth callback.
//! It is only intended for CLI use, not browser environments.
//!
//! Port notes: `node:crypto`'s `randomBytes` is `rand`, which every target has, so `createState` never throws
//! "OpenAI Codex OAuth is only available in Node.js environments". `atob` is the forgiving-base64 decoder of the
//! HTML spec, so a base64url JWT payload (`-` or `_`) still fails to decode as in TS. `response.json()` is
//! `response.text()` plus `JSON.parse`. A truthy non-string token field is kept as its `String(value)`. The
//! credential serializes `refresh` before `access` (see `auth/types.rs`).

use std::rc::Rc;

use base64::Engine;
use base64::engine::{DecodePaddingMode, GeneralPurpose, GeneralPurposeConfig};
use bytes::Bytes;
use indexmap::IndexMap;
use pi_host::{AbortSignal, HttpRequest, HttpResponse, PiError, PiResult};
use rand::RngCore;
use serde_json::{Value, json};

use crate::auth::oauth::callback_server::{
	CallbackOrManualInput, ManualInputPrompt, OAuthCallbackServerOptions, start_oauth_callback_server,
	wait_for_callback_or_manual_input,
};
use crate::auth::oauth::device_code::{
	OAuthDeviceCodePollOptions, OAuthDeviceCodePollResult, poll_oauth_device_code_flow,
};
use crate::auth::oauth::pkce::generate_pkce;
use crate::auth::types::{
	AuthEvent, AuthPrompt, AuthPromptKind, AuthSelectOption, LoginOptions, ModelAuth, OAuthAuth, OAuthCredential,
	ProviderAuthInteraction,
};
use crate::utils::json_parse::json_parse;
use crate::utils::provider_env::get_provider_env_value;
use crate::utils::text::js_trim;

const CLIENT_ID: &str = "app_EMoamEEZ73f0CkXaXp7hrann";
const AUTHORIZE_URL: &str = "https://auth.openai.com/oauth/authorize";
const TOKEN_URL: &str = "https://auth.openai.com/oauth/token";
const REDIRECT_URI: &str = "http://localhost:1455/auth/callback";
const DEVICE_USER_CODE_URL: &str = "https://auth.openai.com/api/accounts/deviceauth/usercode";
const DEVICE_TOKEN_URL: &str = "https://auth.openai.com/api/accounts/deviceauth/token";
const DEVICE_VERIFICATION_URI: &str = "https://auth.openai.com/codex/device";
const DEVICE_REDIRECT_URI: &str = "https://auth.openai.com/deviceauth/callback";
const DEVICE_CODE_TIMEOUT_SECONDS: f64 = 15.0 * 60.0;
const OPENAI_CODEX_BROWSER_LOGIN_METHOD: &str = "browser";
const OPENAI_CODEX_DEVICE_CODE_LOGIN_METHOD: &str = "device_code";
const SCOPE: &str = "openid profile email offline_access";
const JWT_CLAIM_PATH: &str = "https://api.openai.com/auth";

struct OAuthToken {
	access: String,
	refresh: String,
	expires: f64,
}

#[derive(Clone, Copy)]
enum TokenOperation {
	Exchange,
	Refresh,
}

impl TokenOperation {
	fn as_str(self) -> &'static str {
		match self {
			Self::Exchange => "exchange",
			Self::Refresh => "refresh",
		}
	}
}

fn get_callback_host() -> String {
	get_provider_env_value("PI_OAUTH_CALLBACK_HOST", None).unwrap_or_else(|| "127.0.0.1".into())
}

#[derive(Clone)]
struct DeviceAuthInfo {
	device_auth_id: String,
	user_code: String,
	interval_seconds: f64,
}

#[derive(Clone)]
struct DeviceTokenSuccess {
	authorization_code: String,
	code_verifier: String,
}

fn create_state() -> String {
	let mut bytes = [0u8; 16];
	rand::rng().fill_bytes(&mut bytes);
	hex::encode(bytes)
}

#[derive(Debug, Default)]
struct AuthorizationInput {
	code: Option<String>,
	state: Option<String>,
}

fn search_param(pairs: url::form_urlencoded::Parse<'_>, name: &str) -> Option<String> {
	pairs
		.into_iter()
		.find(|(key, _)| key == name)
		.map(|(_, value)| value.into_owned())
}

fn parse_authorization_input(input: &str) -> AuthorizationInput {
	let value = js_trim(input);
	if value.is_empty() {
		return AuthorizationInput::default();
	}

	if let Ok(url) = ::url::Url::parse(value) {
		return AuthorizationInput {
			code: search_param(url.query_pairs(), "code"),
			state: search_param(url.query_pairs(), "state"),
		};
	}

	if value.contains('#') {
		let mut parts = value.split('#');
		let code = parts.next().map(str::to_string);
		let state = parts.next().map(str::to_string);
		return AuthorizationInput { code, state };
	}

	if value.contains("code=") {
		let query = value.strip_prefix('?').unwrap_or(value).as_bytes();
		return AuthorizationInput {
			code: search_param(url::form_urlencoded::parse(query), "code"),
			state: search_param(url::form_urlencoded::parse(query), "state"),
		};
	}

	AuthorizationInput {
		code: Some(value.to_string()),
		state: None,
	}
}

/// `atob(value)` as a byte string: the HTML forgiving-base64 decode, or `None` where `atob` throws.
fn atob(value: &str) -> Option<String> {
	let mut data: String = value
		.chars()
		.filter(|ch| !matches!(ch, '\t' | '\n' | '\u{c}' | '\r' | ' '))
		.collect();
	if data.len().is_multiple_of(4) {
		if data.ends_with("==") {
			data.truncate(data.len() - 2);
		} else if data.ends_with('=') {
			data.truncate(data.len() - 1);
		}
	}
	if data.len() % 4 == 1
		|| !data
			.chars()
			.all(|ch| ch.is_ascii_alphanumeric() || ch == '+' || ch == '/')
	{
		return None;
	}
	let engine = GeneralPurpose::new(
		&base64::alphabet::STANDARD,
		GeneralPurposeConfig::new()
			.with_decode_allow_trailing_bits(true)
			.with_decode_padding_mode(DecodePaddingMode::RequireNone),
	);
	let bytes = engine.decode(data).ok()?;
	Some(bytes.into_iter().map(char::from).collect())
}

fn decode_jwt(token: &str) -> Option<Value> {
	let parts: Vec<&str> = token.split('.').collect();
	if parts.len() != 3 {
		return None;
	}
	let payload = parts[1];
	let decoded = atob(payload)?;
	json_parse(&decoded).ok()
}

async fn fetch_with_login_cancellation(request: HttpRequest) -> PiResult<HttpResponse> {
	let signal = request.signal.clone();
	match pi_host::fetch(request, None).await {
		Ok(response) => Ok(response),
		Err(error) => {
			if signal.as_ref().is_some_and(AbortSignal::aborted) {
				return Err(PiError::new("Login cancelled"));
			}
			Err(error)
		}
	}
}

fn form_request(url: &str, body: &[(&str, &str)], signal: &AbortSignal) -> HttpRequest {
	let mut headers = IndexMap::new();
	headers.insert(
		"Content-Type".to_string(),
		"application/x-www-form-urlencoded".to_string(),
	);
	let body = url::form_urlencoded::Serializer::new(String::new())
		.extend_pairs(body)
		.finish();
	HttpRequest {
		method: "POST".into(),
		url: url.into(),
		headers,
		body: Some(Bytes::from(body)),
		signal: Some(signal.clone()),
		..Default::default()
	}
}

fn json_request(url: &str, body: Value, signal: &AbortSignal) -> HttpRequest {
	let mut headers = IndexMap::new();
	headers.insert("Content-Type".to_string(), "application/json".to_string());
	HttpRequest {
		method: "POST".into(),
		url: url.into(),
		headers,
		body: Some(Bytes::from(pi_host::js::json_stringify(&body))),
		signal: Some(signal.clone()),
		..Default::default()
	}
}

fn js_truthy(value: Option<&Value>) -> bool {
	match value {
		None | Some(Value::Null) => false,
		Some(Value::Bool(value)) => *value,
		Some(Value::Number(number)) => number.as_f64().is_some_and(|number| number != 0.0 && !number.is_nan()),
		Some(Value::String(value)) => !value.is_empty(),
		Some(_) => true,
	}
}

/// JS `String(value)` of a truthy JSON value.
fn js_string(value: &Value) -> String {
	match value {
		Value::String(value) => value.clone(),
		Value::Number(number) => pi_host::js::number_to_string(number.as_f64().unwrap_or(f64::NAN)),
		Value::Array(_) => "[object Array]".into(),
		Value::Object(_) => "[object Object]".into(),
		other => other.to_string(),
	}
}

/// `json?.[key]`.
fn field<'a>(json: &'a Value, key: &str) -> Option<&'a Value> {
	json.as_object().and_then(|object| object.get(key))
}

async fn read_token_response(response: HttpResponse, operation: TokenOperation) -> PiResult<OAuthToken> {
	if !response.ok() {
		let status = response.status;
		let status_text = response.status_text.clone();
		let text = response.text().await.unwrap_or_default();
		return Err(PiError::new(format!(
			"OpenAI Codex token {} failed ({status}): {}",
			operation.as_str(),
			if text.is_empty() { status_text } else { text }
		)));
	}

	let json = json_parse(&response.text().await?)?;
	let access_token = field(&json, "access_token");
	let refresh_token = field(&json, "refresh_token");
	let expires_in = field(&json, "expires_in").and_then(Value::as_f64);
	let (true, true, Some(expires_in)) = (js_truthy(access_token), js_truthy(refresh_token), expires_in) else {
		return Err(PiError::new(format!(
			"OpenAI Codex token {} response missing fields: {}",
			operation.as_str(),
			pi_host::js::json_stringify(&json)
		)));
	};

	Ok(OAuthToken {
		access: js_string(access_token.expect("checked truthy")),
		refresh: js_string(refresh_token.expect("checked truthy")),
		expires: pi_host::now_ms() + expires_in * 1000.0,
	})
}

async fn exchange_authorization_code(
	code: &str,
	verifier: &str,
	redirect_uri: &str,
	signal: &AbortSignal,
) -> PiResult<OAuthToken> {
	let response = fetch_with_login_cancellation(form_request(
		TOKEN_URL,
		&[
			("grant_type", "authorization_code"),
			("client_id", CLIENT_ID),
			("code", code),
			("code_verifier", verifier),
			("redirect_uri", redirect_uri),
		],
		signal,
	))
	.await?;

	read_token_response(response, TokenOperation::Exchange).await
}

async fn refresh_access_token(refresh_token: &str, signal: &AbortSignal) -> PiResult<OAuthToken> {
	let response = pi_host::fetch(
		form_request(
			TOKEN_URL,
			&[
				("grant_type", "refresh_token"),
				("refresh_token", refresh_token),
				("client_id", CLIENT_ID),
			],
			signal,
		),
		None,
	)
	.await
	.map_err(|error| PiError::new(format!("OpenAI Codex token refresh error: {}", error.message)))?;

	read_token_response(response, TokenOperation::Refresh).await
}

async fn start_openai_codex_device_auth(signal: &AbortSignal) -> PiResult<DeviceAuthInfo> {
	let response = fetch_with_login_cancellation(json_request(
		DEVICE_USER_CODE_URL,
		json!({ "client_id": CLIENT_ID }),
		signal,
	))
	.await?;

	if !response.ok() {
		if response.status == 404 {
			return Err(PiError::new(
				"OpenAI Codex device code login is not enabled for this server. Use browser login or verify the server URL.",
			));
		}
		let status = response.status;
		let response_body = response.text().await.unwrap_or_default();
		return Err(PiError::new(format!(
			"OpenAI Codex device code request failed with status {status}{}",
			if response_body.is_empty() {
				String::new()
			} else {
				format!(": {response_body}")
			}
		)));
	}

	let json = json_parse(&response.text().await?)?;
	let interval_seconds = match field(&json, "interval") {
		Some(Value::String(interval)) => Some(pi_host::js::string_to_number(js_trim(interval))),
		Some(Value::Number(interval)) => interval.as_f64(),
		_ => None,
	};
	let device_auth_id = field(&json, "device_auth_id");
	let user_code = field(&json, "user_code");
	let valid = js_truthy(device_auth_id)
		&& js_truthy(user_code)
		&& interval_seconds.is_some_and(|interval| interval.is_finite() && interval >= 0.0);
	if !valid {
		return Err(PiError::new(format!(
			"Invalid OpenAI Codex device code response: {}",
			pi_host::js::json_stringify(&json)
		)));
	}

	Ok(DeviceAuthInfo {
		device_auth_id: js_string(device_auth_id.expect("checked truthy")),
		user_code: js_string(user_code.expect("checked truthy")),
		interval_seconds: interval_seconds.expect("checked finite"),
	})
}

async fn poll_openai_codex_device_auth(device: DeviceAuthInfo, signal: &AbortSignal) -> PiResult<DeviceTokenSuccess> {
	let poll_signal = signal.clone();
	let poll_device = device.clone();
	poll_oauth_device_code_flow(OAuthDeviceCodePollOptions {
		interval_seconds: Some(device.interval_seconds),
		expires_in_seconds: Some(DEVICE_CODE_TIMEOUT_SECONDS),
		wait_before_first_poll: None,
		signal: signal.clone(),
		poll: Rc::new(move || {
			let signal = poll_signal.clone();
			let device = poll_device.clone();
			Box::pin(async move {
				let response = fetch_with_login_cancellation(json_request(
					DEVICE_TOKEN_URL,
					json!({
						"device_auth_id": device.device_auth_id,
						"user_code": device.user_code,
					}),
					&signal,
				))
				.await?;

				if response.ok() {
					let json = json_parse(&response.text().await?)?;
					let authorization_code = field(&json, "authorization_code");
					let code_verifier = field(&json, "code_verifier");
					if !js_truthy(authorization_code) || !js_truthy(code_verifier) {
						return Ok(OAuthDeviceCodePollResult::Failed {
							message: format!(
								"Invalid OpenAI Codex device auth token response: {}",
								pi_host::js::json_stringify(&json)
							),
						});
					}
					return Ok(OAuthDeviceCodePollResult::Complete {
						value: DeviceTokenSuccess {
							authorization_code: js_string(authorization_code.expect("checked truthy")),
							code_verifier: js_string(code_verifier.expect("checked truthy")),
						},
					});
				}

				if response.status == 403 || response.status == 404 {
					return Ok(OAuthDeviceCodePollResult::Pending);
				}

				let status = response.status;
				let response_body = response.text().await.unwrap_or_default();
				let error_code = json_parse(&response_body)
					.ok()
					.and_then(|json| match field(&json, "error") {
						Some(Value::Object(error)) => error.get("code").cloned(),
						Some(error) => Some(error.clone()),
						None => None,
					});

				if error_code.as_ref().and_then(Value::as_str) == Some("deviceauth_authorization_pending") {
					return Ok(OAuthDeviceCodePollResult::Pending);
				}
				if error_code.as_ref().and_then(Value::as_str) == Some("slow_down") {
					return Ok(OAuthDeviceCodePollResult::SlowDown { interval_seconds: None });
				}

				Ok(OAuthDeviceCodePollResult::Failed {
					message: format!(
						"OpenAI Codex device auth failed with status {status}{}",
						if response_body.is_empty() {
							String::new()
						} else {
							format!(": {response_body}")
						}
					),
				})
			})
		}),
	})
	.await
}

struct AuthorizationFlow {
	verifier: String,
	state: String,
	url: String,
}

async fn create_authorization_flow(originator: Option<&str>) -> AuthorizationFlow {
	let originator = originator.unwrap_or("pi");
	let pkce = generate_pkce().await;
	let state = create_state();

	let mut url = ::url::Url::parse(AUTHORIZE_URL).expect("AUTHORIZE_URL is valid");
	url.query_pairs_mut()
		.append_pair("response_type", "code")
		.append_pair("client_id", CLIENT_ID)
		.append_pair("redirect_uri", REDIRECT_URI)
		.append_pair("scope", SCOPE)
		.append_pair("code_challenge", &pkce.challenge)
		.append_pair("code_challenge_method", "S256")
		.append_pair("state", &state)
		.append_pair("id_token_add_organizations", "true")
		.append_pair("codex_cli_simplified_flow", "true")
		.append_pair("originator", originator);

	AuthorizationFlow {
		verifier: pkce.verifier,
		state,
		url: url.to_string(),
	}
}

fn get_account_id(access_token: &str) -> Option<String> {
	let payload = decode_jwt(access_token)?;
	let auth = field(&payload, JWT_CLAIM_PATH)?;
	match field(auth, "chatgpt_account_id") {
		Some(Value::String(account_id)) if !account_id.is_empty() => Some(account_id.clone()),
		_ => None,
	}
}

fn credentials_from_token(token: OAuthToken) -> PiResult<OAuthCredential> {
	let Some(account_id) = get_account_id(&token.access) else {
		return Err(PiError::new("Failed to extract accountId from token"));
	};

	let mut credential = OAuthCredential::new(token.refresh, token.access, token.expires);
	credential.set_field("accountId", Value::String(account_id));
	Ok(credential)
}

async fn exchange_authorization_code_for_credentials(
	code: &str,
	verifier: &str,
	redirect_uri: &str,
	signal: &AbortSignal,
) -> PiResult<OAuthCredential> {
	credentials_from_token(exchange_authorization_code(code, verifier, redirect_uri, signal).await?)
}

async fn login_openai_codex_device_code(interaction: ProviderAuthInteraction) -> PiResult<OAuthCredential> {
	let device = start_openai_codex_device_auth(&interaction.signal).await?;
	interaction.notify(AuthEvent::DeviceCode {
		user_code: device.user_code.clone(),
		verification_uri: DEVICE_VERIFICATION_URI.into(),
		open_browser_on_confirm: None,
		copy_code_on_confirm: None,
		interval_seconds: Some(device.interval_seconds),
		expires_in_seconds: Some(DEVICE_CODE_TIMEOUT_SECONDS),
	});
	let code = poll_openai_codex_device_auth(device, &interaction.signal).await?;
	exchange_authorization_code_for_credentials(
		&code.authorization_code,
		&code.code_verifier,
		DEVICE_REDIRECT_URI,
		&interaction.signal,
	)
	.await
}

async fn login_openai_codex(
	interaction: ProviderAuthInteraction,
	options: Option<LoginOptions>,
) -> PiResult<OAuthCredential> {
	let AuthorizationFlow { verifier, state, url } =
		create_authorization_flow(options.as_ref().and_then(|options| options.agent_name.as_deref())).await;
	// Port 1455 is shared with the Codex CLI; when it is taken, fall back to the pasted redirect URL.
	let callback = start_oauth_callback_server(OAuthCallbackServerOptions {
		provider_name: "OpenAI".into(),
		host: get_callback_host(),
		port: 1455,
		path: "/auth/callback".into(),
		redirect_host: None,
		state: Some(state.clone()),
		complete: Rc::new(|code| Box::pin(async move { Ok(code) })),
		signal: Some(interaction.signal.clone()),
		timeout_ms: None,
	})
	.await
	.ok();

	interaction.notify(AuthEvent::AuthUrl {
		url,
		instructions: Some("A browser window should open. Complete login to finish.".into()),
	});

	let result = async {
		let result = wait_for_callback_or_manual_input(
			&interaction,
			callback.as_ref(),
			ManualInputPrompt {
				message: "Complete login in your browser, or paste the authorization code / redirect URL here:".into(),
				placeholder: REDIRECT_URI.into(),
			},
		)
		.await?;
		let code = match result {
			CallbackOrManualInput::Callback { value } => Some(value),
			CallbackOrManualInput::Manual { input } => {
				let parsed = parse_authorization_input(&input);
				if let Some(parsed_state) = &parsed.state
					&& !parsed_state.is_empty()
					&& *parsed_state != state
				{
					return Err(PiError::new("State mismatch"));
				}
				parsed.code
			}
		};

		let Some(code) = code.filter(|code| !code.is_empty()) else {
			return Err(PiError::new("Missing authorization code"));
		};
		exchange_authorization_code_for_credentials(&code, &verifier, REDIRECT_URI, &interaction.signal).await
	}
	.await;
	if let Some(callback) = &callback {
		callback.close();
	}
	result
}

/// Refresh OpenAI Codex OAuth token
async fn refresh_openai_codex_token(refresh_token: &str, signal: &AbortSignal) -> PiResult<OAuthCredential> {
	credentials_from_token(refresh_access_token(refresh_token, signal).await?)
}

/// `openaiCodexOAuth`.
pub fn openai_codex_oauth() -> OAuthAuth {
	OAuthAuth {
		name: "OpenAI (ChatGPT Plus/Pro)".into(),
		is_subscription: Some(true),
		login_label: None,

		login: Rc::new(|interaction, options| {
			Box::pin(async move {
				let method = interaction
					.prompt(AuthPrompt::new(AuthPromptKind::Select {
						message: "Select OpenAI Codex login method:".into(),
						options: vec![
							AuthSelectOption::new(OPENAI_CODEX_BROWSER_LOGIN_METHOD, "Browser login (default)"),
							AuthSelectOption::new(
								OPENAI_CODEX_DEVICE_CODE_LOGIN_METHOD,
								"Device code login (headless)",
							),
						],
					}))
					.await?;

				if method == OPENAI_CODEX_DEVICE_CODE_LOGIN_METHOD {
					return login_openai_codex_device_code(interaction).await;
				}
				if method != OPENAI_CODEX_BROWSER_LOGIN_METHOD {
					return Err(PiError::new(format!("Unknown OpenAI Codex login method: {method}")));
				}

				login_openai_codex(interaction, options).await
			})
		}),

		refresh: Rc::new(|credential, signal| {
			Box::pin(async move { refresh_openai_codex_token(&credential.refresh, &signal).await })
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
