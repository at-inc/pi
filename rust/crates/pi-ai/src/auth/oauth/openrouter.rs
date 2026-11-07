//! Port of `packages/ai/src/auth/oauth/openrouter.ts`.
//! PORT-STATUS: ported
//!
//! OpenRouter OAuth PKCE flow.
//!
//! OpenRouter exchanges an authorization code for a permanent, user-controlled
//! API key rather than an expiring access/refresh token pair. The callback is
//! handled by a one-shot loopback server on an ephemeral port, raced against a
//! manual prompt so remote/headless sessions can paste the redirect URL when
//! the browser cannot reach the loopback server.
//!
//! NOTE: This module uses node:http (via callback-server.ts) for the OAuth callback server.
//! It is only intended for CLI use, not browser environments.
//!
//! Port notes: the callback server needs the `native` feature, so without it login rejects with the callback
//! server's error, as TS does where `node:http` is missing. `crypto.randomUUID()` is a random version 4 UUID from
//! `rand`. `response.json()` is `response.text()` plus `JSON.parse`. The credential serializes `refresh` before
//! `access` (see `auth/types.rs`).

use std::rc::Rc;

use bytes::Bytes;
use futures::future::{AbortHandle, Abortable};
use indexmap::IndexMap;
use pi_host::abort::ListenerId;
use pi_host::{AbortController, AbortSignal, HttpRequest, PiError, PiResult};
use serde_json::{Map, Value, json};

use crate::auth::oauth::callback_server::{
	CallbackOrManualInput, ManualInputPrompt, OAuthCallbackServerOptions, start_oauth_callback_server,
	wait_for_callback_or_manual_input,
};
use crate::auth::oauth::pkce::generate_pkce;
use crate::auth::types::{AuthEvent, ModelAuth, OAuthAuth, OAuthCredential, ProviderAuthInteraction};
use crate::utils::json_parse::json_parse;
use crate::utils::provider_env::get_provider_env_value;
use crate::utils::text::js_trim;

const AUTHORIZE_URL: &str = "https://openrouter.ai/auth";
const TOKEN_URL: &str = "https://openrouter.ai/api/v1/auth/keys";
const LOGIN_TIMEOUT_MS: f64 = 5.0 * 60.0 * 1000.0;
const TOKEN_EXCHANGE_TIMEOUT_MS: f64 = 30_000.0;
/// `Number.MAX_SAFE_INTEGER`.
const MAX_SAFE_INTEGER: f64 = 9_007_199_254_740_991.0;

fn get_callback_host() -> String {
	get_provider_env_value("PI_OAUTH_CALLBACK_HOST", None).unwrap_or_else(|| "127.0.0.1".into())
}

type JsonObject = Map<String, Value>;

fn search_param(pairs: url::form_urlencoded::Parse<'_>, name: &str) -> Option<String> {
	pairs
		.into_iter()
		.find(|(key, _)| key == name)
		.map(|(_, value)| value.into_owned())
}

fn parse_authorization_input(input: &str) -> Option<String> {
	let value = js_trim(input);
	if value.is_empty() {
		return None;
	}

	if let Ok(url) = url::Url::parse(value) {
		return search_param(url.query_pairs(), "code");
	}

	if value.contains("code=") {
		let query = value.strip_prefix('?').unwrap_or(value);
		return search_param(url::form_urlencoded::parse(query.as_bytes()), "code");
	}

	Some(value.to_string())
}

fn error_detail(body: &JsonObject) -> Option<String> {
	for key in ["error_description", "message", "error"] {
		if let Some(Value::String(detail)) = body.get(key) {
			return Some(detail.clone());
		}
	}
	match body.get("error") {
		Some(Value::Object(error)) => error.get("message").and_then(Value::as_str).map(str::to_string),
		_ => None,
	}
}

/// The TS `finally` of the exchange: stops the timeout and the abort forwarding, also when the future is dropped.
struct ExchangeCleanup {
	timer: AbortHandle,
	signal: AbortSignal,
	listener: ListenerId,
}

impl Drop for ExchangeCleanup {
	fn drop(&mut self) {
		self.timer.abort();
		self.signal.remove_listener(self.listener);
	}
}

async fn exchange_authorization_code(code: &str, verifier: &str, signal: &AbortSignal) -> PiResult<OAuthCredential> {
	if signal.aborted() {
		return Err(PiError::new("Login cancelled"));
	}
	let controller = AbortController::new();
	let listener = signal.add_listener({
		let controller = controller.clone();
		move |reason| controller.abort_with(reason.clone())
	});
	let (timer, registration) = AbortHandle::new_pair();
	let timeout = pi_host::current().sleep(TOKEN_EXCHANGE_TIMEOUT_MS);
	pi_host::spawn_local({
		let controller = controller.clone();
		async move {
			if Abortable::new(timeout, registration).await.is_ok() {
				controller.abort_with(PiError::new("OpenRouter OAuth token exchange timed out"));
			}
		}
	});
	let cleanup = ExchangeCleanup {
		timer,
		signal: signal.clone(),
		listener,
	};

	let exchanged = request_key(code, verifier, &controller.signal()).await;
	drop(cleanup);
	let (ok, status, body) = match exchanged {
		Ok(exchanged) => exchanged,
		Err(error) => {
			if signal.aborted() {
				return Err(PiError::new("Login cancelled"));
			}
			if controller.signal().aborted() {
				return Err(PiError::new("OpenRouter OAuth token exchange timed out"));
			}
			return Err(error);
		}
	};

	if !ok {
		let detail = error_detail(&body);
		return Err(PiError::new(format!(
			"OpenRouter OAuth key exchange failed (HTTP {status}){}",
			match detail.filter(|detail| !detail.is_empty()) {
				Some(detail) => format!(": {detail}"),
				None => String::new(),
			}
		)));
	}

	let Some(Value::String(key)) = body
		.get("key")
		.filter(|key| key.as_str().is_some_and(|key| !key.is_empty()))
	else {
		return Err(PiError::new("OpenRouter OAuth response carries no \"key\""));
	};

	Ok(OAuthCredential::new("", key.clone(), MAX_SAFE_INTEGER))
}

/// The `fetch` and body read of the exchange: `ok`, `status` and the JSON object body (`{}` otherwise).
async fn request_key(code: &str, verifier: &str, signal: &AbortSignal) -> PiResult<(bool, u16, JsonObject)> {
	let mut headers = IndexMap::new();
	headers.insert("accept".to_string(), "application/json".to_string());
	headers.insert("content-type".to_string(), "application/json".to_string());
	let request = HttpRequest {
		method: "POST".into(),
		url: TOKEN_URL.into(),
		headers,
		body: Some(Bytes::from(pi_host::js::json_stringify(&json!({
			"code": code,
			"code_verifier": verifier,
			"code_challenge_method": "S256",
		})))),
		signal: Some(signal.clone()),
		..Default::default()
	};
	let response = pi_host::fetch(request, None).await?;
	let ok = response.ok();
	let status = response.status;
	let body = match response.text().await.and_then(|text| json_parse(&text)) {
		Ok(Value::Object(body)) => body,
		Ok(_) => JsonObject::new(),
		Err(_) if ok => return Err(PiError::new("OpenRouter OAuth returned invalid JSON")),
		Err(_) => JsonObject::new(),
	};
	Ok((ok, status, body))
}

/// `crypto.randomUUID()`: a random version 4 UUID in lowercase hex.
pub(crate) fn random_uuid() -> String {
	let mut bytes: [u8; 16] = rand::random();
	bytes[6] = (bytes[6] & 0x0f) | 0x40;
	bytes[8] = (bytes[8] & 0x3f) | 0x80;
	let hex = hex::encode(bytes);
	format!(
		"{}-{}-{}-{}-{}",
		&hex[0..8],
		&hex[8..12],
		&hex[12..16],
		&hex[16..20],
		&hex[20..32]
	)
}

async fn login_open_router(interaction: ProviderAuthInteraction) -> PiResult<OAuthCredential> {
	let pkce = generate_pkce().await;
	let verifier = pkce.verifier;
	// OpenRouter sends no `state`; the random path keeps stray requests from completing the sign-in.
	let callback = start_oauth_callback_server(OAuthCallbackServerOptions {
		provider_name: "OpenRouter".into(),
		host: get_callback_host(),
		port: 0,
		path: format!("/oauth/callback/{}", random_uuid()),
		redirect_host: None,
		state: None,
		complete: Rc::new({
			let verifier = verifier.clone();
			let signal = interaction.signal.clone();
			move |code| {
				let verifier = verifier.clone();
				let signal = signal.clone();
				Box::pin(async move { exchange_authorization_code(&code, &verifier, &signal).await })
			}
		}),
		signal: Some(interaction.signal.clone()),
		timeout_ms: Some(LOGIN_TIMEOUT_MS),
	})
	.await?;

	let result = async {
		let mut authorize_url = url::Url::parse(AUTHORIZE_URL).expect("AUTHORIZE_URL is valid");
		authorize_url
			.query_pairs_mut()
			.append_pair("callback_url", &callback.redirect_uri)
			.append_pair("code_challenge", &pkce.challenge)
			.append_pair("code_challenge_method", "S256");

		interaction.notify(AuthEvent::Progress {
			message: format!("Listening for OpenRouter OAuth callback on {}", callback.redirect_uri),
		});
		interaction.notify(AuthEvent::AuthUrl {
			url: authorize_url.to_string(),
			instructions: Some(
				"Complete sign-in in your browser. If the browser is on another machine, paste the final redirect URL here."
					.into(),
			),
		});

		let result = wait_for_callback_or_manual_input(
			&interaction,
			Some(&callback),
			ManualInputPrompt {
				message: "Complete sign-in in your browser, or paste the authorization code / redirect URL here:"
					.into(),
				placeholder: callback.redirect_uri.clone(),
			},
		)
		.await?;
		let input = match result {
			CallbackOrManualInput::Callback { value } => return Ok(value),
			CallbackOrManualInput::Manual { input } => input,
		};
		let Some(code) = parse_authorization_input(&input).filter(|code| !code.is_empty()) else {
			return Err(PiError::new("Missing authorization code"));
		};
		interaction.notify(AuthEvent::Progress {
			message: "Exchanging authorization code for an API key...".into(),
		});
		exchange_authorization_code(&code, &verifier, &interaction.signal).await
	}
	.await;
	callback.close();
	result
}

/// `openRouterOAuth`.
pub fn open_router_oauth() -> OAuthAuth {
	OAuthAuth {
		name: "OpenRouter OAuth".into(),
		is_subscription: None,
		login_label: Some("Sign in with OpenRouter".into()),
		login: Rc::new(|interaction, _options| Box::pin(login_open_router(interaction))),
		refresh: Rc::new(|credential, _signal| Box::pin(async move { Ok(credential) })),
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
