//! Port of `packages/ai/src/auth/oauth/openai-chatgpt.ts`.
//! PORT-STATUS: reviewed
//!
//! OpenAI Responses API token sharing through Sign in with ChatGPT.
//!
//! This public-client flow uses no client secret and sends the resulting user
//! access token directly to api.openai.com.
//!
//! Port notes: the callback server is `node:http` in TS and needs the `native` feature here (the shared
//! `callback_server::loopback` server, with Node's binding, parsing and `close()`/`closeAllConnections()`
//! behaviour); without it login rejects with `OAuth callback server requires the native feature`, as TS does where
//! `node:http` is missing. `CALLBACK_HOST` is read when a login starts, not when the module loads. `randomBytes` is
//! `rand`. `response.json()` is `response.text()` plus `JSON.parse`. The credential serializes `refresh` before
//! `access` (see `auth/types.rs`).

use std::rc::Rc;

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use bytes::Bytes;
use futures::future::{Either, select};
use indexmap::IndexMap;
use pi_host::{AbortController, AbortSignal, HttpRequest, PiError, PiResult};
use serde_json::{Map, Value};

use crate::auth::oauth::pkce::generate_pkce;
use crate::auth::types::{
	AuthEvent, AuthPrompt, AuthPromptKind, LoginOptions, ModelAuth, OAuthAuth, OAuthCredential, ProviderAuthInteraction,
};
use crate::utils::json_parse::json_parse;
use crate::utils::provider_env::get_provider_env_value;
use crate::utils::text::js_trim;

// every login registers a new client with this ID; OpenAI returns the issued client ID in the callback
const DYNAMIC_CLIENT_ID: &str = "dynamic_agent_client";
const AGENT_NAME_HINT: &str = "Pi";
const AUTHORIZE_URL: &str = "https://auth.openai.com/api/accounts/authorize";
const TOKEN_URL: &str = "https://auth.openai.com/api/accounts/oauth/token";
const RESOURCE: &str = "https://api.openai.com/v1";
const CALLBACK_PORT: u16 = 1455;
const REDIRECT_URI: &str = "http://127.0.0.1:1455/auth/callback";
const DIRECT_TOKEN_SCOPE: &str = "chatgpt.tokens.use.direct";
const SCOPE: &str = "openid profile email offline_access resource.invoke chatgpt.tokens.use.direct";
// Refresh this long before the real expiry so a request never starts with a token about to expire.
const EXPIRY_MARGIN_MS: f64 = 3.0 * 60.0 * 1000.0;

fn callback_host() -> String {
	get_provider_env_value("PI_OAUTH_CALLBACK_HOST", None).unwrap_or_else(|| "127.0.0.1".into())
}

#[derive(Clone, Debug)]
struct AuthorizationResult {
	code: String,
	client_id: String,
}

type TokenResponse = Map<String, Value>;

fn random_value() -> String {
	let bytes: [u8; 32] = rand::random();
	URL_SAFE_NO_PAD.encode(bytes)
}

fn search_param(url: &url::Url, name: &str) -> Option<String> {
	url.query_pairs()
		.find(|(key, _)| key == name)
		.map(|(_, value)| value.into_owned())
}

fn authorization_result_from_callback(url: &url::Url, expected_state: &str) -> PiResult<AuthorizationResult> {
	let Some(code) = search_param(url, "code").filter(|code| !code.is_empty()) else {
		return Err(PiError::new("Missing authorization code"));
	};
	let Some(state) = search_param(url, "state").filter(|state| !state.is_empty()) else {
		return Err(PiError::new("Missing OAuth state"));
	};
	if state != expected_state {
		return Err(PiError::new("OAuth state mismatch"));
	}
	let client_id = search_param(url, "client_id").map(|client_id| js_trim(&client_id).to_string());
	let Some(client_id) = client_id.filter(|client_id| !client_id.is_empty()) else {
		return Err(PiError::new(
			"OpenAI OAuth registration callback did not contain an issued client ID",
		));
	};
	Ok(AuthorizationResult { code, client_id })
}

fn authorization_result_from_manual_input(input: &str, expected_state: &str) -> PiResult<AuthorizationResult> {
	let Ok(url) = url::Url::parse(js_trim(input)) else {
		return Err(PiError::new("Paste the full callback URL from the browser"));
	};
	let expected = url::Url::parse(REDIRECT_URI).expect("REDIRECT_URI is valid");
	if url.origin() != expected.origin() || url.path() != expected.path() {
		return Err(PiError::new(format!(
			"The pasted callback URL must start with {REDIRECT_URI}"
		)));
	}
	if let Some(error) = search_param(&url, "error").filter(|error| !error.is_empty()) {
		return Err(PiError::new(format!("ChatGPT authorization failed: {error}")));
	}
	authorization_result_from_callback(&url, expected_state)
}

#[cfg(feature = "native")]
mod server {
	use std::rc::Rc;

	use futures::channel::oneshot;
	use pi_host::{LocalBoxFuture, PiError, PiResult};

	use super::{AuthorizationResult, CALLBACK_PORT, REDIRECT_URI, authorization_result_from_callback, search_param};
	use crate::auth::oauth::callback_server::loopback::{LoopbackRequest, LoopbackResponse, LoopbackServer, listen};
	use crate::utils::oauth_page::{oauth_error_html, oauth_success_html};

	const CALLBACK_PATH: &str = "/auth/callback";

	type AuthorizationSender = Rc<std::cell::RefCell<Option<oneshot::Sender<PiResult<AuthorizationResult>>>>>;

	/// The first settlement wins, like a promise's `resolve`/`reject`.
	fn settle(sender: &AuthorizationSender, result: PiResult<AuthorizationResult>) {
		if let Some(sender) = sender.borrow_mut().take() {
			let _ = sender.send(result);
		}
	}

	pub(super) struct CallbackServer {
		pub server: LoopbackServer,
		pub result: oneshot::Receiver<PiResult<AuthorizationResult>>,
	}

	fn send_html(status: u16, body: String) -> LoopbackResponse {
		LoopbackResponse {
			status,
			headers: vec![("Content-Type", "text/html; charset=utf-8")],
			body,
		}
	}

	fn handle(request: &LoopbackRequest, expected_state: &str, result: &AuthorizationSender) -> LoopbackResponse {
		let Ok(url) = url::Url::parse(REDIRECT_URI).and_then(|base| base.join(&request.target)) else {
			return send_html(
				500,
				oauth_error_html("Internal error while processing the callback.", None),
			);
		};
		if url.path() != CALLBACK_PATH {
			return send_html(404, oauth_error_html("Callback route not found.", None));
		}

		if let Some(error) = search_param(&url, "error").filter(|error| !error.is_empty()) {
			let response = send_html(
				400,
				oauth_error_html("ChatGPT was not connected.", Some(&format!("Error: {error}"))),
			);
			settle(
				result,
				Err(PiError::new(format!("ChatGPT authorization failed: {error}"))),
			);
			return response;
		}

		let authorization_result = match authorization_result_from_callback(&url, expected_state) {
			Ok(authorization_result) => authorization_result,
			Err(error) => return send_html(400, oauth_error_html(&error.message, None)),
		};

		let response = send_html(
			200,
			oauth_success_html("ChatGPT authentication completed. You can close this window."),
		);
		settle(result, Ok(authorization_result));
		response
	}

	pub(super) async fn start_callback_server(host: &str, expected_state: &str) -> PiResult<CallbackServer> {
		let (sender, receiver) = oneshot::channel();
		let result: AuthorizationSender = Rc::new(std::cell::RefCell::new(Some(sender)));
		let expected_state = expected_state.to_string();
		let handler = {
			let result = result.clone();
			Rc::new(move |request: LoopbackRequest| {
				let response = handle(&request, &expected_state, &result);
				Box::pin(async move { response }) as LocalBoxFuture<'static, LoopbackResponse>
			})
		};
		let on_error = Rc::new(move |error: PiError| settle(&result, Err(error)));
		let server = listen(host, CALLBACK_PORT, handler, on_error).await?;
		Ok(CallbackServer {
			server,
			result: receiver,
		})
	}
}

#[cfg(not(feature = "native"))]
mod server {
	use futures::channel::oneshot;
	use pi_host::{PiError, PiResult};

	use super::AuthorizationResult;

	pub(super) struct LoopbackServer;

	impl LoopbackServer {
		pub(super) fn close(&self) {}

		pub(super) fn close_all_connections(&self) {}
	}

	pub(super) struct CallbackServer {
		pub server: LoopbackServer,
		pub result: oneshot::Receiver<PiResult<AuthorizationResult>>,
	}

	pub(super) async fn start_callback_server(_host: &str, _expected_state: &str) -> PiResult<CallbackServer> {
		Err(PiError::new("OAuth callback server requires the native feature"))
	}
}

async fn request_token(body: &[(&str, &str)], signal: &AbortSignal) -> PiResult<TokenResponse> {
	let mut headers = IndexMap::new();
	headers.insert("accept".to_string(), "application/json".to_string());
	headers.insert(
		"content-type".to_string(),
		"application/x-www-form-urlencoded".to_string(),
	);
	let body = url::form_urlencoded::Serializer::new(String::new())
		.extend_pairs(body)
		.finish();
	let response = pi_host::fetch(
		HttpRequest {
			method: "POST".into(),
			url: TOKEN_URL.into(),
			headers,
			body: Some(Bytes::from(body)),
			signal: Some(signal.clone()),
			..Default::default()
		},
		None,
	)
	.await?;
	if !response.ok() {
		let status = response.status;
		let status_text = response.status_text.clone();
		let response_body = response.text().await.unwrap_or_default();
		return Err(PiError::new(format!(
			"OpenAI OAuth token request failed ({status}): {}",
			if response_body.is_empty() {
				status_text
			} else {
				response_body
			}
		)));
	}
	match json_parse(&response.text().await?)? {
		Value::Object(data) => Ok(data),
		_ => Err(PiError::new("OpenAI OAuth token response must be an object")),
	}
}

fn require_token_string(value: Option<&Value>, field: &str) -> PiResult<String> {
	match value {
		Some(Value::String(value)) if !js_trim(value).is_empty() => Ok(value.clone()),
		_ => Err(PiError::new(format!("OpenAI OAuth token response has invalid {field}"))),
	}
}

/// JS `\s`: the white space `String.prototype.trim` removes.
fn is_js_whitespace(ch: char) -> bool {
	(ch.is_whitespace() && ch != '\u{85}') || ch == '\u{feff}'
}

fn credential_from_token_response(token: &TokenResponse, client_id: &str) -> PiResult<OAuthCredential> {
	let access = require_token_string(token.get("access_token"), "access_token")?;
	let refresh = require_token_string(token.get("refresh_token"), "refresh_token")?;
	let scope = require_token_string(token.get("scope"), "scope")?;
	let Some(expires_in) = token
		.get("expires_in")
		.and_then(Value::as_f64)
		.filter(|expires_in| expires_in.is_finite() && *expires_in > 0.0)
	else {
		return Err(PiError::new("OpenAI OAuth token response has invalid expires_in"));
	};
	let scopes: Vec<&str> = js_trim(&scope)
		.split(is_js_whitespace)
		.filter(|scope| !scope.is_empty())
		.collect();
	if !scopes.contains(&DIRECT_TOKEN_SCOPE) {
		return Err(PiError::new(format!(
			"OpenAI OAuth grant did not include {DIRECT_TOKEN_SCOPE}"
		)));
	}
	let mut credential = OAuthCredential::new(
		refresh,
		access,
		pi_host::now_ms() + expires_in * 1000.0 - EXPIRY_MARGIN_MS,
	);
	credential.set_field("clientId", Value::String(client_id.to_string()));
	credential.set_field(
		"scopes",
		Value::Array(
			scopes
				.into_iter()
				.map(|scope| Value::String(scope.to_string()))
				.collect(),
		),
	);
	Ok(credential)
}

async fn exchange_authorization_code(
	code: &str,
	verifier: &str,
	client_id: &str,
	signal: &AbortSignal,
) -> PiResult<OAuthCredential> {
	let token = request_token(
		&[
			("grant_type", "authorization_code"),
			("client_id", client_id),
			("code", code),
			("code_verifier", verifier),
			("redirect_uri", REDIRECT_URI),
			("resource", RESOURCE),
		],
		signal,
	)
	.await?;
	// Pi does not use the ID token to identify the user or read profile data.
	// Keep the presence check as part of the token-response contract.
	if !matches!(token.get("id_token"), Some(Value::String(id_token)) if !js_trim(id_token).is_empty()) {
		return Err(PiError::new("OpenAI OAuth token response did not contain an ID token"));
	}
	credential_from_token_response(&token, client_id)
}

async fn refresh_access_token(credential: OAuthCredential, signal: AbortSignal) -> PiResult<OAuthCredential> {
	let client_id = match credential.field("clientId") {
		Some(Value::String(client_id)) if !js_trim(client_id).is_empty() => client_id.clone(),
		_ => {
			return Err(PiError::new(
				"Stored OpenAI OAuth credential does not contain an issued client ID; reconnect ChatGPT",
			));
		}
	};
	let token = request_token(
		&[
			("grant_type", "refresh_token"),
			("client_id", &client_id),
			("refresh_token", &credential.refresh),
			("resource", RESOURCE),
		],
		&signal,
	)
	.await?;
	credential_from_token_response(&token, &client_id)
}

/// OpenAI identifies each installation ("agent host") by a stable URI such as `urn:uuid:<uuid>`.
fn agent_host_id(device_id: Option<String>) -> PiResult<String> {
	match device_id {
		Some(device_id) if is_uuid(&device_id) => Ok(format!("urn:uuid:{}", device_id.to_lowercase())),
		_ => Err(PiError::new(
			"Sign in with ChatGPT requires a device ID (UUID) for this installation",
		)),
	}
}

/// `/^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i`.
fn is_uuid(value: &str) -> bool {
	let groups: Vec<&str> = value.split('-').collect();
	groups.len() == 5
		&& groups
			.iter()
			.zip([8, 4, 4, 4, 12])
			.all(|(group, length)| group.len() == length && group.bytes().all(|byte| byte.is_ascii_hexdigit()))
}

async fn login_openai_chatgpt(
	interaction: ProviderAuthInteraction,
	options: Option<LoginOptions>,
) -> PiResult<OAuthCredential> {
	let device_id = options
		.as_ref()
		.and_then(|options| options.get_device_id.as_ref())
		.map(|get_device_id| get_device_id());
	let host_id = agent_host_id(device_id)?;
	let pkce = generate_pkce().await;
	let state = random_value();
	let nonce = random_value();
	// Without this server, the browser's callback would reach whatever else holds the port (another
	// pending login or the Codex CLI), which rejects it as a state mismatch. Fail with a clear error instead.
	let callback = server::start_callback_server(&callback_host(), &state)
		.await
		.map_err(|error| {
			if error.data.as_ref().and_then(|data| data.get("code")).and_then(Value::as_str) != Some("EADDRINUSE") {
				return error;
			}
			PiError::new(format!(
				"Port {CALLBACK_PORT} is in use, probably by an unfinished login in another pi session or by the Codex CLI. Cancel that login and try again."
			))
		})?;

	let mut authorization_url = url::Url::parse(AUTHORIZE_URL).expect("AUTHORIZE_URL is valid");
	authorization_url
		.query_pairs_mut()
		.append_pair("client_id", DYNAMIC_CLIENT_ID)
		.append_pair(
			"agent_name_hint",
			options
				.as_ref()
				.and_then(|options| options.agent_name.as_deref())
				.unwrap_or(AGENT_NAME_HINT),
		)
		.append_pair("ext_agent_host_id", &host_id)
		.append_pair("response_type", "code")
		.append_pair("redirect_uri", REDIRECT_URI)
		.append_pair("resource", RESOURCE)
		.append_pair("scope", SCOPE)
		.append_pair("state", &state)
		.append_pair("code_challenge", &pkce.challenge)
		.append_pair("code_challenge_method", "S256")
		.append_pair("nonce", &nonce);
	interaction.notify(AuthEvent::AuthUrl {
		url: authorization_url.to_string(),
		instructions: Some(
			"Complete sign-in in your browser. If the callback does not complete, paste the final redirect URL here."
				.into(),
		),
	});

	let manual_abort = AbortController::new();
	let manual_code = {
		let prompt = interaction.prompt(AuthPrompt {
			signal: Some(AbortSignal::any(&[manual_abort.signal(), interaction.signal.clone()])),
			kind: AuthPromptKind::ManualCode {
				message: "Complete login in your browser, or paste the final redirect URL here:".into(),
				placeholder: Some(REDIRECT_URI.into()),
			},
		});
		let state = state.clone();
		async move { authorization_result_from_manual_input(&prompt.await?, &state) }
	};

	let server::CallbackServer { server, result } = callback;
	let outcome = async {
		let callback_result = async {
			result
				.await
				.unwrap_or_else(|_| Err(PiError::new("OAuth callback server closed")))
		};
		futures::pin_mut!(callback_result, manual_code);
		let result = match select(callback_result, manual_code).await {
			Either::Left((result, _)) | Either::Right((result, _)) => result?,
		};
		interaction.notify(AuthEvent::Progress {
			message: "Exchanging authorization code for tokens...".into(),
		});
		exchange_authorization_code(&result.code, &pkce.verifier, &result.client_id, &interaction.signal).await
	}
	.await;
	manual_abort.abort();
	server.close();
	// close() only stops accepting new connections. Browsers open spare connections ahead of
	// time, and one that has not sent a request yet stays open and attached to this server.
	// A later login in the same process starts a new server with a new state, but the browser
	// may send that login's callback over the spare connection. This server would then handle
	// it and reject it with "OAuth state mismatch", and the new login would never see it.
	server.close_all_connections();
	match outcome {
		Err(_) if interaction.signal.aborted() => Err(PiError::new("Login cancelled")),
		outcome => outcome,
	}
}

/// `openaiChatGPTOAuth`.
pub fn openai_chat_gpt_oauth() -> OAuthAuth {
	OAuthAuth {
		name: "OpenAI (ChatGPT subscription)".into(),
		is_subscription: Some(true),
		login_label: Some("Sign in with ChatGPT".into()),
		login: Rc::new(|interaction, options| Box::pin(login_openai_chatgpt(interaction, options))),
		refresh: Rc::new(|credential, signal| Box::pin(refresh_access_token(credential, signal))),
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
