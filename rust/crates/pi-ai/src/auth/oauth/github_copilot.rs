//! Port of `packages/ai/src/auth/oauth/github-copilot.ts`.
//! PORT-STATUS: reviewed
//!
//! GitHub Copilot OAuth flow
//!
//! Port notes: `response.json()` is `response.text()` plus `JSON.parse`; a JSON array body counts as an object, as
//! `typeof [] === "object"` in TS. `await response.body?.cancel()` is dropping the response. A request without
//! `method` is a GET. The credential serializes `refresh, access, expires` before `enterpriseUrl` and
//! `availableModelIds` (see `auth/types.rs`). An enterprise domain is `Option<String>`; TS truthiness checks on it
//! are `is_some_and(|domain| !domain.is_empty())`.

use std::rc::Rc;

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use bytes::Bytes;
use indexmap::{IndexMap, IndexSet};
use pi_host::{AbortSignal, HttpRequest, HttpResponse, PiError, PiResult};
use serde_json::Value;

use crate::auth::oauth::device_code::{
	OAuthDeviceCodePollOptions, OAuthDeviceCodePollResult, poll_oauth_device_code_flow,
};
use crate::auth::oauth::trusted_url::trusted_http_url;
use crate::auth::types::{
	AuthEvent, AuthPrompt, AuthPromptKind, ModelAuth, OAuthAuth, OAuthCredential, ProviderAuthInteraction,
};
use crate::providers::github_copilot_models::GITHUB_COPILOT_MODELS;
use crate::utils::json_parse::json_parse;
use crate::utils::sleep::sleep;
use crate::utils::text::js_trim;

fn decode(value: &str) -> String {
	String::from_utf8(STANDARD.decode(value).expect("CLIENT_ID is base64")).expect("CLIENT_ID is UTF-8")
}

fn client_id() -> String {
	decode("SXYxLmI1MDdhMDhjODdlY2ZlOTg=")
}

const COPILOT_HEADERS: [(&str, &str); 4] = [
	("User-Agent", "GitHubCopilotChat/0.35.0"),
	("Editor-Version", "vscode/1.107.0"),
	("Editor-Plugin-Version", "copilot-chat/0.35.0"),
	("Copilot-Integration-Id", "vscode-chat"),
];
const COPILOT_API_VERSION: &str = "2026-06-01";

#[derive(Clone, Debug)]
struct DeviceCodeResponse {
	device_code: String,
	user_code: String,
	verification_uri: String,
	interval: Option<f64>,
	expires_in: f64,
}

/// `{ maxRetries, maxElapsedMs }` of `fetchWithRateLimitRetry`.
#[derive(Clone, Copy, Debug)]
struct RetryPolicy {
	max_retries: u32,
	max_elapsed_ms: f64,
}

/// The `RequestInit` fields this flow sends.
#[derive(Clone, Debug, Default)]
struct RequestInit {
	method: Option<&'static str>,
	headers: Vec<(&'static str, String)>,
	body: Option<String>,
}

fn normalize_domain(input: &str) -> Option<String> {
	let trimmed = js_trim(input);
	if trimmed.is_empty() {
		return None;
	}
	let url = if trimmed.contains("://") {
		url::Url::parse(trimmed)
	} else {
		url::Url::parse(&format!("https://{trimmed}"))
	};
	url.ok().map(|url| url.host_str().unwrap_or_default().to_string())
}

struct Urls {
	device_code_url: String,
	access_token_url: String,
	copilot_token_url: String,
}

fn get_urls(domain: &str) -> Urls {
	Urls {
		device_code_url: format!("https://{domain}/login/device/code"),
		access_token_url: format!("https://{domain}/login/oauth/access_token"),
		copilot_token_url: format!("https://api.{domain}/copilot_internal/v2/token"),
	}
}

/// Parse the proxy-ep from a Copilot token and convert to API base URL.
/// Token format: tid=...;exp=...;proxy-ep=proxy.individual.githubcopilot.com;...
/// Returns API URL like https://api.individual.githubcopilot.com
fn get_base_url_from_token(token: &str) -> Option<String> {
	// `/proxy-ep=([^;]+)/`: the first "proxy-ep=" followed by at least one character other than ";".
	let proxy_host = token.match_indices("proxy-ep=").find_map(|(index, marker)| {
		let rest = &token[index + marker.len()..];
		let value = &rest[..rest.find(';').unwrap_or(rest.len())];
		(!value.is_empty()).then_some(value)
	})?;
	// Convert proxy.xxx to api.xxx
	let api_host = match proxy_host.strip_prefix("proxy.") {
		Some(rest) => format!("api.{rest}"),
		None => proxy_host.to_string(),
	};
	Some(format!("https://{api_host}"))
}

fn get_github_copilot_base_url(token: Option<&str>, enterprise_domain: Option<&str>) -> String {
	// If we have a token, extract the base URL from proxy-ep
	if let Some(url_from_token) = token
		.filter(|token| !token.is_empty())
		.and_then(get_base_url_from_token)
	{
		return url_from_token;
	}
	// Fallback for enterprise or if token parsing fails
	if let Some(enterprise_domain) = enterprise_domain.filter(|domain| !domain.is_empty()) {
		return format!("https://copilot-api.{enterprise_domain}");
	}
	"https://api.individual.githubcopilot.com".into()
}

/// `asRecord`: objects and arrays; an array has no named fields, so it reads as an empty record.
fn as_record(value: Option<&Value>) -> Option<&serde_json::Map<String, Value>> {
	static EMPTY: std::sync::LazyLock<serde_json::Map<String, Value>> = std::sync::LazyLock::new(Default::default);
	match value {
		Some(Value::Object(record)) => Some(record),
		Some(Value::Array(_)) => Some(&EMPTY),
		_ => None,
	}
}

struct AccountModel {
	id: String,
	picker_enabled: bool,
	policy_state: Option<Value>,
}

impl AccountModel {
	fn policy_state_is(&self, state: &str) -> bool {
		self.policy_state.as_ref().and_then(Value::as_str) == Some(state)
	}
}

struct GitHubCopilotModelCatalog {
	available_model_ids: Vec<String>,
	policy_model_ids: Vec<String>,
}

fn parse_github_copilot_model_catalog(raw: &Value, allow_policy_fallback: bool) -> PiResult<GitHubCopilotModelCatalog> {
	let Some(Value::Array(data)) = as_record(Some(raw)).and_then(|record| record.get("data")) else {
		return Err(PiError::new("Invalid Copilot models response"));
	};

	let account_models: Vec<AccountModel> = data
		.iter()
		.filter_map(|raw_item| {
			let item = as_record(Some(raw_item))?;
			let Some(Value::String(id)) = item.get("id") else {
				return None;
			};

			let capabilities = as_record(item.get("capabilities"));
			let supports = as_record(capabilities.and_then(|capabilities| capabilities.get("supports")));
			if supports.and_then(|supports| supports.get("tool_calls")) == Some(&Value::Bool(false)) {
				return None;
			}

			Some(AccountModel {
				id: id.clone(),
				picker_enabled: item.get("model_picker_enabled") == Some(&Value::Bool(true)),
				policy_state: as_record(item.get("policy"))
					.and_then(|policy| policy.get("state"))
					.cloned(),
			})
		})
		.collect();
	let picker_model_ids: Vec<String> = account_models
		.iter()
		.filter(|model| model.picker_enabled && !model.policy_state_is("disabled"))
		.map(|model| model.id.clone())
		.collect();
	let use_policy_fallback = allow_policy_fallback && picker_model_ids.is_empty();
	let policy_model_ids = account_models
		.iter()
		.filter(|model| {
			model.policy_state_is("unconfigured")
				&& GITHUB_COPILOT_MODELS.contains_key(&model.id)
				&& (model.picker_enabled || use_policy_fallback)
		})
		.map(|model| model.id.clone())
		.collect();
	let available_model_ids = if !picker_model_ids.is_empty() || !allow_policy_fallback {
		picker_model_ids
	} else {
		account_models
			.iter()
			.filter(|model| model.policy_state_is("enabled"))
			.map(|model| model.id.clone())
			.collect()
	};
	Ok(GitHubCopilotModelCatalog {
		available_model_ids,
		policy_model_ids,
	})
}

async fn fetch(url: &str, init: &RequestInit, signal: AbortSignal) -> PiResult<HttpResponse> {
	let headers: IndexMap<String, String> = init
		.headers
		.iter()
		.map(|(name, value)| (name.to_string(), value.clone()))
		.collect();
	pi_host::fetch(
		HttpRequest {
			method: init.method.unwrap_or("GET").into(),
			url: url.into(),
			headers,
			body: init.body.clone().map(Bytes::from),
			signal: Some(signal),
			..Default::default()
		},
		None,
	)
	.await
}

async fn fetch_with_rate_limit_retry(
	url: &str,
	init: RequestInit,
	signal: &AbortSignal,
	retry_policy: RetryPolicy,
) -> PiResult<HttpResponse> {
	let retry_budget_signal = (retry_policy.max_retries > 0 && retry_policy.max_elapsed_ms > 0.0)
		.then(|| AbortSignal::timeout(retry_policy.max_elapsed_ms));
	let request_signal = match &retry_budget_signal {
		Some(retry_budget_signal) => AbortSignal::any(&[signal.clone(), retry_budget_signal.clone()]),
		None => signal.clone(),
	};
	let retry_deadline = retry_budget_signal
		.is_some()
		.then(|| pi_host::now_ms() + retry_policy.max_elapsed_ms);
	let mut retry = 0;
	loop {
		let response = fetch(
			url,
			&init,
			AbortSignal::any(&[request_signal.clone(), AbortSignal::timeout(5000.0)]),
		)
		.await?;
		if response.status != 429 || retry == retry_policy.max_retries {
			return Ok(response);
		}

		let mut delay_ms = 500.0 * 2f64.powi(retry as i32);
		if let Some(retry_after) = response.header("retry-after").filter(|value| !value.is_empty()) {
			let seconds = pi_host::js::parse_float(retry_after);
			delay_ms = if seconds.is_nan() {
				pi_host::js::date_parse(retry_after) - pi_host::now_ms()
			} else {
				seconds * 1000.0
			};
			if !delay_ms.is_finite() {
				return Ok(response);
			}
		}
		delay_ms = pi_host::js::math_max(0.0, delay_ms);
		if retry_deadline.is_some_and(|retry_deadline| delay_ms >= retry_deadline - pi_host::now_ms()) {
			return Ok(response);
		}
		drop(response);
		sleep(delay_ms, &request_signal).await?;
		retry += 1;
	}
}

/// `{ ...headers, ...COPILOT_HEADERS, ...extra }`.
fn with_copilot_headers(
	headers: Vec<(&'static str, String)>,
	extra: Vec<(&'static str, String)>,
) -> Vec<(&'static str, String)> {
	headers
		.into_iter()
		.chain(COPILOT_HEADERS.iter().map(|(name, value)| (*name, value.to_string())))
		.chain(extra)
		.collect()
}

async fn status_error(response: HttpResponse) -> PiError {
	let status = response.status;
	let status_text = response.status_text.clone();
	let text = response.text().await.unwrap_or_default();
	PiError::new(format!("{status} {status_text}: {text}"))
}

async fn fetch_github_copilot_models(
	copilot_token: &str,
	enterprise_domain: Option<&str>,
	signal: &AbortSignal,
	retry_policy: RetryPolicy,
) -> PiResult<GitHubCopilotModelCatalog> {
	let base_url = get_github_copilot_base_url(Some(copilot_token), enterprise_domain);
	// Some Individual accounts return false for every picker flag despite explicit enabled policies.
	// Limit the fallback to that endpoint so other account types keep strict picker semantics.
	let allow_policy_fallback = base_url == "https://api.individual.githubcopilot.com";
	let headers = with_copilot_headers(
		vec![
			("Accept", "application/json".to_string()),
			("Authorization", format!("Bearer {copilot_token}")),
		],
		vec![("X-GitHub-Api-Version", COPILOT_API_VERSION.to_string())],
	);
	let response = fetch_with_rate_limit_retry(
		&format!("{base_url}/models"),
		RequestInit {
			headers,
			..RequestInit::default()
		},
		signal,
		retry_policy,
	)
	.await?;
	if !response.ok() {
		return Err(status_error(response).await);
	}
	parse_github_copilot_model_catalog(&json_parse(&response.text().await?)?, allow_policy_fallback)
}

async fn fetch_json(url: &str, init: RequestInit, signal: &AbortSignal) -> PiResult<Value> {
	let response = fetch(url, &init, signal.clone()).await?;
	if !response.ok() {
		return Err(status_error(response).await);
	}
	json_parse(&response.text().await?)
}

fn form_body(pairs: &[(&str, &str)]) -> String {
	url::form_urlencoded::Serializer::new(String::new())
		.extend_pairs(pairs)
		.finish()
}

fn device_flow_headers() -> Vec<(&'static str, String)> {
	vec![
		("Accept", "application/json".into()),
		("Content-Type", "application/x-www-form-urlencoded".into()),
		("User-Agent", "GitHubCopilotChat/0.35.0".into()),
	]
}

/// `value && typeof value === "object"`.
fn is_object(value: &Value) -> bool {
	matches!(value, Value::Object(_) | Value::Array(_))
}

async fn start_device_flow(domain: &str, signal: &AbortSignal) -> PiResult<DeviceCodeResponse> {
	let urls = get_urls(domain);
	let data = fetch_json(
		&urls.device_code_url,
		RequestInit {
			method: Some("POST"),
			headers: device_flow_headers(),
			body: Some(form_body(&[("client_id", &client_id()), ("scope", "read:user")])),
		},
		signal,
	)
	.await?;

	if !is_object(&data) {
		return Err(PiError::new("Invalid device code response"));
	}

	let field = |key: &str| data.get(key);
	let (
		Some(Value::String(device_code)),
		Some(Value::String(user_code)),
		Some(verification_uri @ Value::String(_)),
		interval @ (None | Some(Value::Number(_))),
		Some(Value::Number(expires_in)),
	) = (
		field("device_code"),
		field("user_code"),
		field("verification_uri"),
		field("interval"),
		field("expires_in"),
	)
	else {
		return Err(PiError::new("Invalid device code response fields"));
	};

	let Some(trusted_verification_uri) = trusted_http_url(verification_uri) else {
		return Err(PiError::new("Untrusted verification_uri in device code response"));
	};

	Ok(DeviceCodeResponse {
		device_code: device_code.clone(),
		user_code: user_code.clone(),
		verification_uri: trusted_verification_uri,
		interval: interval.and_then(Value::as_f64),
		expires_in: expires_in.as_f64().unwrap_or_default(),
	})
}

/// Template-literal interpolation of a truthy JSON value.
fn js_template_value(value: &Value) -> String {
	match value {
		Value::String(value) => value.clone(),
		Value::Number(value) => pi_host::js::number_to_string(value.as_f64().unwrap_or_default()),
		Value::Bool(value) => value.to_string(),
		Value::Array(values) => values
			.iter()
			.map(|value| match value {
				Value::Null => String::new(),
				value => js_template_value(value),
			})
			.collect::<Vec<_>>()
			.join(","),
		Value::Object(_) => "[object Object]".into(),
		Value::Null => "null".into(),
	}
}

/// JS truthiness of a JSON value.
fn is_truthy(value: &Value) -> bool {
	match value {
		Value::Null => false,
		Value::Bool(value) => *value,
		Value::Number(value) => value.as_f64().is_some_and(|value| value != 0.0),
		Value::String(value) => !value.is_empty(),
		Value::Array(_) | Value::Object(_) => true,
	}
}

async fn poll_for_github_access_token(
	domain: &str,
	device: &DeviceCodeResponse,
	signal: &AbortSignal,
) -> PiResult<String> {
	let access_token_url = get_urls(domain).access_token_url;
	let device_code = device.device_code.clone();
	let poll_signal = signal.clone();
	poll_oauth_device_code_flow(OAuthDeviceCodePollOptions {
		interval_seconds: device.interval,
		expires_in_seconds: Some(device.expires_in),
		wait_before_first_poll: Some(true),
		signal: signal.clone(),
		poll: Rc::new(move || {
			let access_token_url = access_token_url.clone();
			let device_code = device_code.clone();
			let signal = poll_signal.clone();
			Box::pin(async move {
				let raw = fetch_json(
					&access_token_url,
					RequestInit {
						method: Some("POST"),
						headers: device_flow_headers(),
						body: Some(form_body(&[
							("client_id", &client_id()),
							("device_code", &device_code),
							("grant_type", "urn:ietf:params:oauth:grant-type:device_code"),
						])),
					},
					&signal,
				)
				.await?;

				if let Some(Value::String(access_token)) = raw.get("access_token") {
					return Ok(OAuthDeviceCodePollResult::Complete {
						value: access_token.clone(),
					});
				}

				if let Some(Value::String(error)) = raw.get("error") {
					if error == "authorization_pending" {
						return Ok(OAuthDeviceCodePollResult::Pending);
					}

					if error == "slow_down" {
						return Ok(OAuthDeviceCodePollResult::SlowDown {
							interval_seconds: raw.get("interval").and_then(Value::as_f64),
						});
					}

					let description_suffix = match raw.get("error_description") {
						Some(description) if is_truthy(description) => format!(": {}", js_template_value(description)),
						_ => String::new(),
					};
					return Ok(OAuthDeviceCodePollResult::Failed {
						message: format!("Device flow failed: {error}{description_suffix}"),
					});
				}

				Ok(OAuthDeviceCodePollResult::Failed {
					message: "Invalid device token response".into(),
				})
			})
		}),
	})
	.await
}

async fn refresh_github_copilot_access_token(
	refresh_token: &str,
	enterprise_domain: Option<&str>,
	signal: &AbortSignal,
) -> PiResult<OAuthCredential> {
	let domain = enterprise_domain
		.filter(|domain| !domain.is_empty())
		.unwrap_or("github.com");
	let urls = get_urls(domain);

	let headers = with_copilot_headers(
		vec![
			("Accept", "application/json".to_string()),
			("Authorization", format!("Bearer {refresh_token}")),
		],
		Vec::new(),
	);
	let raw = fetch_json(
		&urls.copilot_token_url,
		RequestInit {
			headers,
			..RequestInit::default()
		},
		signal,
	)
	.await?;

	if !is_object(&raw) {
		return Err(PiError::new("Invalid Copilot token response"));
	}

	let (Some(Value::String(token)), Some(Value::Number(expires_at))) = (raw.get("token"), raw.get("expires_at"))
	else {
		return Err(PiError::new("Invalid Copilot token response fields"));
	};

	let mut credential = OAuthCredential::new(
		refresh_token,
		token.clone(),
		expires_at.as_f64().unwrap_or_default() * 1000.0 - 5.0 * 60.0 * 1000.0,
	);
	if let Some(enterprise_domain) = enterprise_domain {
		credential.set_field("enterpriseUrl", Value::String(enterprise_domain.to_string()));
	}
	Ok(credential)
}

fn string_array(values: Vec<String>) -> Value {
	Value::Array(values.into_iter().map(Value::String).collect())
}

/// Refresh GitHub Copilot token
async fn refresh_github_copilot_token(
	refresh_token: String,
	enterprise_domain: Option<String>,
	signal: AbortSignal,
) -> PiResult<OAuthCredential> {
	let mut credentials =
		refresh_github_copilot_access_token(&refresh_token, enterprise_domain.as_deref(), &signal).await?;
	let GitHubCopilotModelCatalog {
		available_model_ids, ..
	} = fetch_github_copilot_models(
		&credentials.access,
		enterprise_domain.as_deref(),
		&signal,
		RetryPolicy {
			max_retries: 0,
			max_elapsed_ms: 0.0,
		},
	)
	.await?;
	credentials.set_field("availableModelIds", string_array(available_model_ids));
	Ok(credentials)
}

/// Enable a model for the user's GitHub Copilot account.
/// This is required for some models (like Claude, Grok) before they can be used.
async fn enable_github_copilot_model(
	token: &str,
	model_id: &str,
	enterprise_domain: Option<&str>,
	signal: &AbortSignal,
) -> PiResult<bool> {
	let base_url = get_github_copilot_base_url(Some(token), enterprise_domain);
	let url = format!("{base_url}/models/{model_id}/policy");

	let headers = with_copilot_headers(
		vec![
			("Content-Type", "application/json".to_string()),
			("Authorization", format!("Bearer {token}")),
		],
		vec![
			("openai-intent", "chat-policy".into()),
			("x-interaction-type", "chat-policy".into()),
		],
	);
	let response = match fetch_with_rate_limit_retry(
		&url,
		RequestInit {
			method: Some("POST"),
			headers,
			body: Some(pi_host::js::json_stringify(&serde_json::json!({ "state": "enabled" }))),
		},
		signal,
		RetryPolicy {
			max_retries: 2,
			max_elapsed_ms: 5000.0,
		},
	)
	.await
	{
		Ok(response) => response,
		Err(error) => {
			if signal.aborted() {
				return Err(error);
			}
			return Ok(false);
		}
	};
	if response.status == 429 {
		return Err(status_error(response).await);
	}
	Ok(response.ok())
}

/// Enable the requested GitHub Copilot models and return the successful IDs.
/// Policy updates are best effort; exhausted rate limiting stops the batch.
async fn enable_github_copilot_models(
	token: &str,
	model_ids: &[String],
	enterprise_domain: Option<&str>,
	signal: &AbortSignal,
) -> PiResult<Vec<String>> {
	let mut enabled_model_ids = Vec::new();
	for model_id in model_ids {
		match enable_github_copilot_model(token, model_id, enterprise_domain, signal).await {
			Ok(true) => enabled_model_ids.push(model_id.clone()),
			Ok(false) => {}
			Err(error) => {
				if signal.aborted() {
					return Err(error);
				}
				break;
			}
		}
	}
	Ok(enabled_model_ids)
}

async fn login_github_copilot(interaction: ProviderAuthInteraction) -> PiResult<OAuthCredential> {
	let input = interaction
		.prompt(AuthPrompt::new(AuthPromptKind::Text {
			message: "GitHub Enterprise URL/domain (blank for github.com)".into(),
			placeholder: Some("company.ghe.com".into()),
		}))
		.await?;
	if interaction.signal.aborted() {
		return Err(PiError::new("Login cancelled"));
	}

	let trimmed = js_trim(&input);
	let enterprise_domain = normalize_domain(&input).filter(|domain| !domain.is_empty());
	if !trimmed.is_empty() && enterprise_domain.is_none() {
		return Err(PiError::new("Invalid GitHub Enterprise URL/domain"));
	}
	let enterprise_domain = enterprise_domain.as_deref();
	let domain = enterprise_domain.unwrap_or("github.com");

	let device = start_device_flow(domain, &interaction.signal).await?;
	interaction.notify(AuthEvent::DeviceCode {
		user_code: device.user_code.clone(),
		verification_uri: device.verification_uri.clone(),
		open_browser_on_confirm: None,
		copy_code_on_confirm: None,
		interval_seconds: device.interval,
		expires_in_seconds: Some(device.expires_in),
	});

	let github_access_token = poll_for_github_access_token(domain, &device, &interaction.signal).await?;
	let mut credentials =
		refresh_github_copilot_access_token(&github_access_token, enterprise_domain, &interaction.signal).await?;
	let models = fetch_github_copilot_models(
		&credentials.access,
		enterprise_domain,
		&interaction.signal,
		RetryPolicy {
			max_retries: 2,
			max_elapsed_ms: 5000.0,
		},
	)
	.await?;
	let mut enabled_model_ids = Vec::new();
	if !models.policy_model_ids.is_empty() {
		interaction.notify(AuthEvent::Progress {
			message: "Enabling models...".into(),
		});
		enabled_model_ids = enable_github_copilot_models(
			&credentials.access,
			&models.policy_model_ids,
			enterprise_domain,
			&interaction.signal,
		)
		.await?;
	}
	let available_model_ids: IndexSet<String> = models
		.available_model_ids
		.into_iter()
		.chain(enabled_model_ids)
		.collect();
	credentials.set_field(
		"availableModelIds",
		string_array(available_model_ids.into_iter().collect()),
	);
	Ok(credentials)
}

fn copilot_enterprise_domain(credential: &OAuthCredential) -> Option<String> {
	match credential.field("enterpriseUrl") {
		Some(Value::String(enterprise_url)) if !enterprise_url.is_empty() => normalize_domain(enterprise_url),
		_ => None,
	}
}

/// `githubCopilotOAuth`.
pub fn github_copilot_oauth() -> OAuthAuth {
	OAuthAuth {
		name: "GitHub Copilot".into(),
		is_subscription: Some(true),
		login_label: None,
		login: Rc::new(|interaction, _options| Box::pin(login_github_copilot(interaction))),
		refresh: Rc::new(|credential, signal| {
			let enterprise_domain = copilot_enterprise_domain(&credential);
			Box::pin(refresh_github_copilot_token(
				credential.refresh,
				enterprise_domain,
				signal,
			))
		}),
		// Derive the credential-specific proxy endpoint for each request.
		to_auth: Rc::new(|credential| {
			Box::pin(async move {
				let base_url = get_github_copilot_base_url(
					Some(&credential.access),
					copilot_enterprise_domain(&credential).as_deref(),
				);
				Ok(ModelAuth {
					api_key: Some(credential.access),
					base_url: Some(base_url),
					..ModelAuth::default()
				})
			})
		}),
	}
}
