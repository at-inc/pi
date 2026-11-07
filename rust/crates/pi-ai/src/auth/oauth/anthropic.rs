//! Port of `packages/ai/src/auth/oauth/anthropic.ts`.
//! PORT-STATUS: reviewed
//!
//! Anthropic OAuth flow (Claude Pro/Max)
//!
//! NOTE: This module uses Node.js http.createServer for the OAuth callback server.
//! It is only intended for CLI use, not browser environments.
//!
//! Port notes: `CALLBACK_HOST` is read when a login starts instead of once at module load. `OAuthAuth` objects hold
//! closures, so the `anthropicOAuth` constant is the function `anthropic_oauth()`. Token fields of a token response
//! that are not strings become `""` where TS keeps `undefined` (which `JSON.stringify` drops). Rust errors carry no
//! stack, so `formatErrorDetails` never adds `stack=`.

use std::rc::Rc;

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use bytes::Bytes;
use indexmap::IndexMap;
use pi_host::{AbortSignal, HttpRequest, PiError, PiResult};
use serde_json::{Value, json};

use crate::auth::oauth::callback_server::{
	CallbackOrManualInput, ManualInputPrompt, OAuthCallbackServer, OAuthCallbackServerOptions,
	start_oauth_callback_server, wait_for_callback_or_manual_input,
};
use crate::auth::oauth::pkce::generate_pkce;
use crate::auth::types::{
	AuthEvent, AuthPrompt, AuthPromptKind, AuthSelectOption, ModelAuth, OAuthAuth, OAuthCredential,
	ProviderAuthInteraction,
};
use crate::utils::json_parse::json_parse;
use crate::utils::provider_env::get_provider_env_value;

fn decode(s: &str) -> String {
	String::from_utf8_lossy(&STANDARD.decode(s).expect("CLIENT_ID is valid base64")).into_owned()
}

fn client_id() -> String {
	decode("OWQxYzI1MGEtZTYxYi00NGQ5LTg4ZWQtNTk0NGQxOTYyZjVl")
}

const AUTHORIZE_URL: &str = "https://claude.ai/oauth/authorize";
const TOKEN_URL: &str = "https://platform.claude.com/v1/oauth/token";

fn callback_host() -> String {
	get_provider_env_value("PI_OAUTH_CALLBACK_HOST", None).unwrap_or_else(|| "127.0.0.1".into())
}

// Preferred so the port can be forwarded into containers or over SSH. Anthropic accepts any loopback port,
// so login falls back to a free port when this one cannot be bound (#10571).
const CALLBACK_PORT: u16 = 53692;
const CALLBACK_PATH: &str = "/callback";
const REDIRECT_URI: &str = "http://localhost:53692/callback";
const COPY_CODE_REDIRECT_URI: &str = "https://platform.claude.com/oauth/code/callback";
const ANTHROPIC_BROWSER_LOGIN_METHOD: &str = "browser";
const ANTHROPIC_COPY_CODE_LOGIN_METHOD: &str = "copy_code";
const SCOPES: &str =
	"org:create_api_key user:profile user:inference user:sessions:claude_code user:mcp_servers user:file_upload";

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
	let value = crate::utils::text::js_trim(input);
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

fn format_error_details(error: &PiError) -> String {
	let mut details = vec![format!("{}: {}", error.name, error.message)];
	push_error_fields(&mut details, error.data.as_ref());
	details.join("; ")
}

/// The `code`, `errno` and `cause` parts of `formatErrorDetails`, read from `PiError.data`.
fn push_error_fields(details: &mut Vec<String>, data: Option<&Value>) {
	let Some(data) = data else {
		return;
	};
	if let Some(code) = data.get("code").filter(|code| js_truthy(code)) {
		details.push(format!("code={}", js_string(code)));
	}
	if let Some(errno) = data.get("errno") {
		details.push(format!("errno={}", js_string(errno)));
	}
	if let Some(cause) = data.get("cause") {
		details.push(format!("cause={}", format_cause_details(cause)));
	}
}

/// `formatErrorDetails(error.cause)`: a cause stored as `{ name, message, data? }` is an `Error`.
fn format_cause_details(cause: &Value) -> String {
	match (
		cause.get("name").and_then(Value::as_str),
		cause.get("message").and_then(Value::as_str),
	) {
		(Some(name), Some(message)) => {
			let mut details = vec![format!("{name}: {message}")];
			push_error_fields(&mut details, cause.get("data"));
			details.join("; ")
		}
		_ => js_string(cause),
	}
}

fn js_truthy(value: &Value) -> bool {
	match value {
		Value::Null => false,
		Value::Bool(value) => *value,
		Value::Number(number) => number.as_f64().is_some_and(|number| number != 0.0 && !number.is_nan()),
		Value::String(value) => !value.is_empty(),
		Value::Array(_) | Value::Object(_) => true,
	}
}

/// JS `String(value)`.
fn js_string(value: &Value) -> String {
	match value {
		Value::String(value) => value.clone(),
		Value::Number(number) => pi_host::js::number_to_string(number.as_f64().unwrap_or(f64::NAN)),
		Value::Array(_) => "[object Array]".into(),
		Value::Object(_) => "[object Object]".into(),
		other => other.to_string(),
	}
}

/// JS `Number(value)` for a JSON value or a missing property.
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

/// `value.key` of a parsed JSON value; reading a property of `null` throws like V8.
fn property<'a>(value: &'a Value, key: &str) -> PiResult<Option<&'a Value>> {
	if value.is_null() {
		return Err(PiError::type_error(format!(
			"Cannot read properties of null (reading '{key}')"
		)));
	}
	Ok(value.get(key))
}

fn string_field(value: Option<&Value>) -> String {
	value.and_then(Value::as_str).unwrap_or_default().to_string()
}

async fn post_json(url: &str, body: Value, signal: &AbortSignal) -> PiResult<String> {
	let mut headers = IndexMap::new();
	headers.insert("Content-Type".to_string(), "application/json".to_string());
	headers.insert("Accept".to_string(), "application/json".to_string());
	let response = pi_host::fetch(
		HttpRequest {
			method: "POST".into(),
			url: url.into(),
			headers,
			body: Some(Bytes::from(pi_host::js::json_stringify(&body))),
			signal: Some(AbortSignal::any(&[signal.clone(), AbortSignal::timeout(30_000.0)])),
			..Default::default()
		},
		None,
	)
	.await?;

	let status = response.status;
	let ok = response.ok();
	let response_body = response.text().await?;

	if !ok {
		return Err(PiError::new(format!(
			"HTTP request failed. status={status}; url={url}; body={response_body}"
		)));
	}

	Ok(response_body)
}

async fn exchange_authorization_code(
	code: &str,
	state: &str,
	verifier: &str,
	redirect_uri: &str,
	signal: &AbortSignal,
) -> PiResult<OAuthCredential> {
	let response_body = post_json(
		TOKEN_URL,
		json!({
			"grant_type": "authorization_code",
			"client_id": client_id(),
			"code": code,
			"state": state,
			"redirect_uri": redirect_uri,
			"code_verifier": verifier,
		}),
		signal,
	)
	.await
	.map_err(|error| {
		PiError::new(format!(
			"Token exchange request failed. url={TOKEN_URL}; redirect_uri={redirect_uri}; response_type=authorization_code; details={}",
			format_error_details(&error)
		))
	})?;

	let token_data = json_parse(&response_body).map_err(|error| {
		PiError::new(format!(
			"Token exchange returned invalid JSON. url={TOKEN_URL}; body={response_body}; details={}",
			format_error_details(&error)
		))
	})?;

	let refresh = string_field(property(&token_data, "refresh_token")?);
	let access = string_field(property(&token_data, "access_token")?);
	let expires_in = js_number(property(&token_data, "expires_in")?);
	Ok(OAuthCredential::new(
		refresh,
		access,
		pi_host::now_ms() + expires_in * 1000.0 - 5.0 * 60.0 * 1000.0,
	))
}

async fn start_callback_server(
	port: u16,
	verifier: &str,
	signal: &AbortSignal,
) -> PiResult<OAuthCallbackServer<String>> {
	start_oauth_callback_server(OAuthCallbackServerOptions {
		provider_name: "Anthropic".into(),
		host: callback_host(),
		port,
		path: CALLBACK_PATH.into(),
		redirect_host: Some("localhost".into()),
		state: Some(verifier.to_string()),
		complete: Rc::new(|code| Box::pin(async move { Ok(code) })),
		signal: Some(signal.clone()),
		timeout_ms: None,
	})
	.await
}

async fn login_anthropic(interaction: ProviderAuthInteraction) -> PiResult<OAuthCredential> {
	let pkce = generate_pkce().await;
	let verifier = pkce.verifier;
	// Without a callback server, login continues with the pasted redirect URL.
	let callback = match start_callback_server(CALLBACK_PORT, &verifier, &interaction.signal).await {
		Ok(callback) => Some(callback),
		Err(_) => start_callback_server(0, &verifier, &interaction.signal).await.ok(),
	};
	let redirect_uri = callback
		.as_ref()
		.map(|callback| callback.redirect_uri.clone())
		.unwrap_or_else(|| REDIRECT_URI.to_string());

	let result = async {
		let auth_params = url::form_urlencoded::Serializer::new(String::new())
			.append_pair("code", "true")
			.append_pair("client_id", &client_id())
			.append_pair("response_type", "code")
			.append_pair("redirect_uri", &redirect_uri)
			.append_pair("scope", SCOPES)
			.append_pair("code_challenge", &pkce.challenge)
			.append_pair("code_challenge_method", "S256")
			.append_pair("state", &verifier)
			// Existing claude.ai sessions otherwise auto-complete and skip account selection.
			.append_pair("prompt", "login")
			.finish();
		interaction.notify(AuthEvent::AuthUrl {
			url: format!("{AUTHORIZE_URL}?{auth_params}"),
			instructions: Some(
				"Complete login in your browser. If the browser is on another machine, paste the final redirect URL here."
					.into(),
			),
		});

		let result = wait_for_callback_or_manual_input(
			&interaction,
			callback.as_ref(),
			ManualInputPrompt {
				message: "Complete login in your browser, or paste the authorization code / redirect URL here:".into(),
				placeholder: redirect_uri.clone(),
			},
		)
		.await?;
		let code;
		let mut state = verifier.clone();
		match result {
			CallbackOrManualInput::Callback { value } => code = Some(value),
			CallbackOrManualInput::Manual { input } => {
				let parsed = parse_authorization_input(&input);
				if let Some(parsed_state) = &parsed.state
					&& !parsed_state.is_empty()
					&& *parsed_state != verifier
				{
					return Err(PiError::new("OAuth state mismatch"));
				}
				code = parsed.code;
				state = parsed.state.unwrap_or_else(|| verifier.clone());
			}
		}

		let Some(code) = code.filter(|code| !code.is_empty()) else {
			return Err(PiError::new("Missing authorization code"));
		};
		interaction.notify(AuthEvent::Progress {
			message: "Exchanging authorization code for tokens...".into(),
		});
		exchange_authorization_code(&code, &state, &verifier, &redirect_uri, &interaction.signal).await
	}
	.await;
	if let Some(callback) = &callback {
		callback.close();
	}
	result
}

async fn login_anthropic_copy_code(interaction: ProviderAuthInteraction) -> PiResult<OAuthCredential> {
	let pkce = generate_pkce().await;
	let verifier = pkce.verifier;
	let auth_params = url::form_urlencoded::Serializer::new(String::new())
		.append_pair("code", "true")
		.append_pair("client_id", &client_id())
		.append_pair("response_type", "code")
		.append_pair("redirect_uri", COPY_CODE_REDIRECT_URI)
		.append_pair("scope", SCOPES)
		.append_pair("code_challenge", &pkce.challenge)
		.append_pair("code_challenge_method", "S256")
		.append_pair("state", &verifier)
		.finish();
	interaction.notify(AuthEvent::AuthUrl {
		url: format!("{AUTHORIZE_URL}?{auth_params}"),
		instructions: Some(
			"Complete login in your browser, then copy the code Anthropic shows and paste it here.".into(),
		),
	});

	let input = interaction
		.prompt(AuthPrompt {
			signal: Some(interaction.signal.clone()),
			kind: AuthPromptKind::ManualCode {
				message: "Paste the code Anthropic shows after you sign in:".into(),
				placeholder: Some("code#state".into()),
			},
		})
		.await?;
	let parsed = parse_authorization_input(&input);
	if let Some(parsed_state) = &parsed.state
		&& !parsed_state.is_empty()
		&& *parsed_state != verifier
	{
		return Err(PiError::new("OAuth state mismatch"));
	}
	let Some(code) = parsed.code.filter(|code| !code.is_empty()) else {
		return Err(PiError::new("Missing authorization code"));
	};
	interaction.notify(AuthEvent::Progress {
		message: "Exchanging authorization code for tokens...".into(),
	});
	exchange_authorization_code(
		&code,
		parsed.state.as_deref().unwrap_or(&verifier),
		&verifier,
		COPY_CODE_REDIRECT_URI,
		&interaction.signal,
	)
	.await
}

/// Refresh Anthropic OAuth token
async fn refresh_anthropic_token(refresh_token: &str, signal: &AbortSignal) -> PiResult<OAuthCredential> {
	let response_body = post_json(
		TOKEN_URL,
		json!({
			"grant_type": "refresh_token",
			"client_id": client_id(),
			"refresh_token": refresh_token,
		}),
		signal,
	)
	.await
	.map_err(|error| {
		PiError::new(format!(
			"Anthropic token refresh request failed. url={TOKEN_URL}; details={}",
			format_error_details(&error)
		))
	})?;

	let data = json_parse(&response_body).map_err(|error| {
		PiError::new(format!(
			"Anthropic token refresh returned invalid JSON. url={TOKEN_URL}; body={response_body}; details={}",
			format_error_details(&error)
		))
	})?;

	let refresh = string_field(property(&data, "refresh_token")?);
	let access = string_field(property(&data, "access_token")?);
	let expires_in = js_number(property(&data, "expires_in")?);
	Ok(OAuthCredential::new(
		refresh,
		access,
		pi_host::now_ms() + expires_in * 1000.0 - 5.0 * 60.0 * 1000.0,
	))
}

/// `anthropicOAuth`.
pub fn anthropic_oauth() -> OAuthAuth {
	OAuthAuth {
		name: "Anthropic (Claude Pro/Max)".into(),
		is_subscription: Some(true),
		login_label: None,

		login: Rc::new(|interaction, _options| {
			Box::pin(async move {
				let method = interaction
					.prompt(AuthPrompt::new(AuthPromptKind::Select {
						message: "Select Anthropic login method:".into(),
						options: vec![
							AuthSelectOption::new(ANTHROPIC_BROWSER_LOGIN_METHOD, "Browser login (default)"),
							AuthSelectOption::new(ANTHROPIC_COPY_CODE_LOGIN_METHOD, "Copy code login (headless)"),
						],
					}))
					.await?;

				if method == ANTHROPIC_COPY_CODE_LOGIN_METHOD {
					return login_anthropic_copy_code(interaction).await;
				}
				if method != ANTHROPIC_BROWSER_LOGIN_METHOD {
					return Err(PiError::new(format!("Unknown Anthropic login method: {method}")));
				}

				login_anthropic(interaction).await
			})
		}),

		refresh: Rc::new(|credential, signal| {
			Box::pin(async move { refresh_anthropic_token(&credential.refresh, &signal).await })
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
