//! Rust-only replacement for the parts of `@anthropic-ai/sdk` 1.1.0 that `api/anthropic-messages.ts` relies on
//! (`BaseAnthropic` in `client.ts`, `internal/headers.ts`, `internal/detect-platform.ts`, `core/error.ts`, and the
//! OIDC federation credentials in `lib/credentials/`). See "Provider SDKs" in `rust/PORTING.md`.
//! PORT-STATUS: ported
//!
//! What it reproduces: `client.beta.messages.create(params, options).asResponse()` for a streaming request (URL,
//! body, default, auth, platform and `x-stainless-*` headers, `buildHeaders` merge and null-suppression rules,
//! `validateHeaders`, the request timeout, the SDK retry policy with `retry-after-ms`/`retry-after`/`x-should-retry`,
//! and `APIError` message formatting), `withOptions({ defaultHeaders })`, `ANTHROPIC_CUSTOM_HEADERS`, and the
//! `oidc_federation` config: identity token file, jwt-bearer token exchange, `TokenCache` refresh policy, 401
//! invalidation, and the `oauth-2025-04-20` beta appended to token-authenticated requests.
//!
//! Differences from the SDK, none of which pi-ai reaches:
//! - Credentials and the base URL are explicit. The client never reads `ANTHROPIC_API_KEY`/`ANTHROPIC_AUTH_TOKEN`/
//!   `ANTHROPIC_BASE_URL` (pi-ai always passes `model.baseUrl`) or runs the default
//!   credential chain (profiles, config files): pi-ai's `PiAnthropic` passes `apiKey: null, authToken: null` and
//!   disables the chain. Only `oidc_federation` configs without `credentials_path` or `base_url` are supported, which
//!   is the config pi-ai builds; `user_oauth`, `credentials`, `profile`, an `apiKey` function, middleware,
//!   OpenTelemetry tracing, logging, `fetchOptions`, `defaultQuery` and the `x-stainless-helper` tagging are not.
//!   `betas`, `user_profile_id` and `workspace_id` are header strings (`String(value)`); the SDK's array form is not.
//! - The deprecated-model and thinking-type `console.warn` calls of `messages.create` are not ported.
//! - Request headers reach `fetch` as a `Headers` object would list them: lowercase names in sorted order, values of
//!   repeated names joined with `", "` and trimmed of HTTP whitespace. Header values are not validated.
//! - Platform headers describe the Rust build like `utils/identity.rs`: `X-Stainless-OS` and `X-Stainless-Arch` use
//!   the SDK's normalization of Node's names, `X-Stainless-Runtime` is `rust` with the crate's `rust-version`. On
//!   wasm32 they are the SDK's `Unknown`/`unknown` fallback.
//! - When the request timeout fires, the request's signal aborts and the call fails at once instead of waiting for a
//!   `fetch` that ignores its signal. The token exchange is sent without `redirect: "manual"`, which `HttpRequest`
//!   cannot express.
//! - Errors are `PiError`s named `Error` (the SDK classes do not set `name`). An `APIError` from a response has the
//!   HTTP status in `PiError.status` and `data` `{ headers, error?, type }` with the response headers as
//!   `Object.fromEntries(headers)`; connection, timeout and user-abort errors have `data.headers: null`, which marks
//!   them as provider errors for `utils/provider_retry.rs`.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use futures::FutureExt;
use futures::channel::oneshot;
use futures::future::{LocalBoxFuture, Shared};
use indexmap::{IndexMap, IndexSet};
use pi_host::js::{json_stringify, number_to_string, utf16_len, utf16_prefix};
use pi_host::{AbortController, AbortSignal, FetchFunction, HttpRequest, HttpResponse, PiError, PiResult};
use serde::Serialize;
use serde_json::{Map, Value, json};

use crate::types::ProviderHeaders;
use crate::utils::headers::headers_to_record;
use crate::utils::json_parse::json_parse;

/// `VERSION` of the SDK this file reproduces.
pub const VERSION: &str = "1.1.0";

/// `BaseAnthropic.DEFAULT_TIMEOUT`: 10 minutes.
pub const DEFAULT_TIMEOUT: f64 = 600000.0;

const DEFAULT_BASE_URL: &str = "https://api.anthropic.com";

pub const GRANT_TYPE_JWT_BEARER: &str = "urn:ietf:params:oauth:grant-type:jwt-bearer";
pub const TOKEN_ENDPOINT: &str = "/v1/oauth/token";

/// `anthropic-beta` value required on authenticated API requests using an
/// OAuth bearer token, and on `refresh_token` grants against the token endpoint.
pub const OAUTH_API_BETA_HEADER: &str = "oauth-2025-04-20";

/// `anthropic-beta` value required on jwt-bearer exchanges against the token
/// endpoint. It routes the request to the federation service; it must NOT be
/// sent on `refresh_token` grants, which are handled by a different backend.
pub const FEDERATION_BETA_HEADER: &str = "oidc-federation-2026-04-01";

pub const ADVISORY_REFRESH_THRESHOLD_IN_SECONDS: f64 = 120.0;
pub const MANDATORY_REFRESH_THRESHOLD_IN_SECONDS: f64 = 30.0;
pub const ADVISORY_REFRESH_BACKOFF_IN_SECONDS: f64 = 5.0;

const MAX_TOKEN_RESPONSE_BYTES: usize = 1 << 20;
const MAX_ERROR_BODY_CHARS: usize = 2000;

/// `identity_token` of an `oidc_federation` config.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct IdentityTokenConfig {
	/// Only `"file"` is supported.
	pub source: String,
	#[serde(skip_serializing_if = "Option::is_none")]
	pub path: Option<String>,
}

/// `authentication` of an `oidc_federation` config.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct OidcFederationAuthentication {
	/// Always `"oidc_federation"`.
	#[serde(rename = "type")]
	pub r#type: String,
	#[serde(skip_serializing_if = "Option::is_none")]
	pub federation_rule_id: Option<String>,
	#[serde(skip_serializing_if = "Option::is_none")]
	pub service_account_id: Option<String>,
	#[serde(skip_serializing_if = "Option::is_none")]
	pub identity_token: Option<IdentityTokenConfig>,
}

/// `ClientOptions["config"]` (`AnthropicConfig`) with `oidc_federation` authentication.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct AnthropicConfig {
	#[serde(skip_serializing_if = "Option::is_none")]
	pub organization_id: Option<String>,
	#[serde(skip_serializing_if = "Option::is_none")]
	pub workspace_id: Option<String>,
	pub authentication: OidcFederationAuthentication,
}

/// `ClientOptions`. `None` credentials are the SDK's explicit `null` (see the file header).
#[derive(Clone, Default)]
pub struct ClientOptions {
	pub api_key: Option<String>,
	pub auth_token: Option<String>,
	pub config: Option<AnthropicConfig>,
	/// Defaults to `https://api.anthropic.com`; an empty string also selects the default.
	pub base_url: Option<String>,
	/// Milliseconds; defaults to [`DEFAULT_TIMEOUT`].
	pub timeout: Option<f64>,
	/// Defaults to 2.
	pub max_retries: Option<f64>,
	/// A `None` value removes a header the SDK would send.
	pub default_headers: Option<ProviderHeaders>,
	pub fetch: Option<FetchFunction>,
	pub dangerously_allow_browser: Option<bool>,
}

/// `RequestOptions` of a single call.
#[derive(Clone, Default)]
pub struct RequestOptions {
	/// A `None` value removes the header.
	pub headers: Option<ProviderHeaders>,
	pub signal: Option<AbortSignal>,
	pub timeout: Option<f64>,
	pub max_retries: Option<f64>,
}

/// `NullableHeaders`: header values by lowercase name, and the names explicitly set to `null`.
#[derive(Clone, Debug, Default)]
struct NullableHeaders {
	values: IndexMap<String, Vec<String>>,
	nulls: IndexSet<String>,
}

impl NullableHeaders {
	/// `headers.get(name)`.
	fn get(&self, name: &str) -> Option<String> {
		self.values
			.get(&name.to_ascii_lowercase())
			.map(|values| values.join(", "))
	}

	/// `headers.entries()`: sorted lowercase names with their combined values.
	fn entries(&self) -> IndexMap<String, String> {
		let mut entries: IndexMap<String, String> = self
			.values
			.iter()
			.map(|(name, values)| (name.clone(), values.join(", ")))
			.collect();
		entries.sort_keys();
		entries
	}

	/// The entries as a header source for [`build_headers`]: values, then nulls.
	fn as_source(&self) -> HeaderSource {
		let mut entries: Vec<(String, Option<String>)> = self
			.entries()
			.into_iter()
			.map(|(name, value)| (name, Some(value)))
			.collect();
		entries.extend(self.nulls.iter().map(|name| (name.clone(), None)));
		HeaderSource { record: false, entries }
	}
}

/// One `buildHeaders` source in iteration order. `record` marks a plain object, whose every key replaces the
/// headers of that name set so far, where an already-built `NullableHeaders` appends repeated names.
struct HeaderSource {
	record: bool,
	entries: Vec<(String, Option<String>)>,
}

impl HeaderSource {
	fn record(entries: Vec<(String, Option<String>)>) -> Self {
		Self { record: true, entries }
	}
}

fn record_source<V: AsRef<str>>(headers: &IndexMap<String, V>) -> HeaderSource {
	HeaderSource::record(
		headers
			.iter()
			.map(|(name, value)| (name.clone(), Some(value.as_ref().to_string())))
			.collect(),
	)
}

fn nullable_source(headers: Option<&ProviderHeaders>) -> HeaderSource {
	HeaderSource::record(
		headers
			.map(|headers| {
				headers
					.iter()
					.map(|(name, value)| (name.clone(), value.clone()))
					.collect()
			})
			.unwrap_or_default(),
	)
}

/// `Headers` value normalization: leading and trailing HTTP whitespace removed.
fn normalize_header_value(value: &str) -> String {
	value.trim_matches([' ', '\t', '\r', '\n']).to_string()
}

/// `buildHeaders(newHeaders)`: later sources replace a header of an earlier one, a header repeated within one built
/// source is appended (a record key always replaces), and `null` removes a header and records the removal.
fn build_headers(new_headers: Vec<HeaderSource>) -> NullableHeaders {
	let mut target = NullableHeaders::default();
	for headers in new_headers {
		let mut seen_headers: IndexSet<String> = IndexSet::new();
		for (name, value) in headers.entries {
			let lower_name = name.to_ascii_lowercase();
			if headers.record || !seen_headers.contains(&lower_name) {
				target.values.shift_remove(&lower_name);
				seen_headers.insert(lower_name.clone());
			}
			match value {
				None => {
					target.values.shift_remove(&lower_name);
					target.nulls.insert(lower_name);
				}
				Some(value) => {
					target
						.values
						.entry(lower_name.clone())
						.or_default()
						.push(normalize_header_value(&value));
					target.nulls.shift_remove(&lower_name);
				}
			}
		}
	}
	target
}

/// JS truthiness of a JSON value.
pub(crate) fn truthy(value: &Value) -> bool {
	match value {
		Value::Null => false,
		Value::Bool(value) => *value,
		Value::Number(number) => number.as_f64().is_some_and(|number| number != 0.0 && !number.is_nan()),
		Value::String(text) => !text.is_empty(),
		Value::Array(_) | Value::Object(_) => true,
	}
}

/// `new AnthropicError(message)`.
fn anthropic_error(message: impl Into<String>) -> PiError {
	PiError::new(message)
}

/// `APIError.makeMessage(status, error, message)`.
fn make_api_error_message(status: Option<u16>, error: Option<&Value>, message: Option<&str>) -> String {
	let msg: Option<String> = match error {
		Some(error) if error.get("message").is_some_and(truthy) => match &error["message"] {
			Value::String(message) => Some(message.clone()),
			message => Some(json_stringify(message)),
		},
		Some(error) if truthy(error) => Some(json_stringify(error)),
		_ => message.map(str::to_string),
	};
	let msg = msg.filter(|msg| !msg.is_empty());
	match (status.filter(|status| *status != 0), msg) {
		(Some(status), Some(msg)) => format!("{status} {msg}"),
		(Some(status), None) => format!("{status} status code (no body)"),
		(None, Some(msg)) => msg,
		(None, None) => "(no status code or body)".into(),
	}
}

/// `APIError.generate(status, errorResponse, message, headers)` for a response.
fn make_status_error(
	status: u16,
	error: Option<Value>,
	message: Option<&str>,
	headers: &IndexMap<String, String>,
) -> PiError {
	let r#type = error
		.as_ref()
		.and_then(|error| error.get("error"))
		.and_then(|error| error.get("type"))
		.cloned()
		.unwrap_or(Value::Null);
	let mut data = Map::new();
	data.insert(
		"headers".into(),
		Value::Object(
			headers_to_record(headers)
				.into_iter()
				.map(|(name, value)| (name, Value::String(value)))
				.collect(),
		),
	);
	if let Some(error) = &error {
		data.insert("error".into(), error.clone());
	}
	data.insert("type".into(), r#type);
	PiError::new(make_api_error_message(Some(status), error.as_ref(), message))
		.with_status(status)
		.with_data(Value::Object(data))
}

/// `new APIUserAbortError()`.
fn user_abort_error() -> PiError {
	PiError::new("Request was aborted.").with_data(json!({ "headers": null }))
}

/// `new APIConnectionError({ cause })`.
fn connection_error(cause: &PiError) -> PiError {
	PiError::new("Connection error.").with_data(json!({
		"headers": null,
		"cause": { "name": cause.name, "message": cause.message },
	}))
}

/// `new APIConnectionTimeoutError()`.
fn connection_timeout_error() -> PiError {
	PiError::new("Request timed out.").with_data(json!({ "headers": null }))
}

/// `new WorkloadIdentityError(message, statusCode, body, requestId)`.
fn workload_identity_error(
	message: impl Into<String>,
	status_code: Option<u16>,
	body: Value,
	request_id: Option<&str>,
) -> PiError {
	PiError::new(message).with_data(json!({
		"statusCode": status_code,
		"body": body,
		"requestId": request_id,
	}))
}

/// `validatePositiveInteger(name, n)`.
fn validate_positive_integer(name: &str, n: f64) -> PiResult<f64> {
	if !n.is_finite() || n.fract() != 0.0 {
		return Err(anthropic_error(format!("{name} must be an integer")));
	}
	if n < 0.0 {
		return Err(anthropic_error(format!("{name} must be a positive integer")));
	}
	Ok(n)
}

/// `readEnv(env)`: trimmed, empty as unset.
fn read_env(name: &str) -> Option<String> {
	pi_host::env(name)
		.map(|value| js_trim(&value).to_string())
		.filter(|value| !value.is_empty())
}

/// `String(error)` of a thrown `Error`.
fn error_string(error: &PiError) -> String {
	if error.message.is_empty() {
		return error.name.clone();
	}
	format!("{}: {}", error.name, error.message)
}

/// `nowAsSeconds()`.
fn now_as_seconds() -> f64 {
	(pi_host::now_ms() / 1000.0).floor()
}

/// `String.prototype.trim`: JS white space and line terminators, which include U+FEFF but not U+0085.
fn js_trim(value: &str) -> &str {
	value.trim_matches(|ch: char| (ch.is_whitespace() && ch != '\u{85}') || ch == '\u{feff}')
}

/// `qs`'s RFC 3986 encoder: every UTF-8 byte except `A-Za-z0-9-_.~` percent-encoded.
fn qs_encode(value: &str) -> String {
	value
		.bytes()
		.map(|byte| match byte {
			b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => (byte as char).to_string(),
			_ => format!("%{byte:02X}"),
		})
		.collect()
}

/// `transformOutputFormat(params)`: the deprecated `output_format` moves to `output_config.format`.
fn transform_output_format(params: &mut Map<String, Value>) -> PiResult<()> {
	if !params.get("output_format").is_some_and(truthy) {
		return Ok(());
	}
	let output_config = params.get("output_config");
	if output_config
		.and_then(|config| config.get("format"))
		.is_some_and(truthy)
	{
		return Err(anthropic_error(
			"Both output_format and output_config.format were provided. Please use only output_config.format (output_format is deprecated).",
		));
	}
	let mut config = match output_config {
		Some(Value::Object(config)) => config.clone(),
		_ => Map::new(),
	};
	let output_format = params.shift_remove("output_format").expect("output_format is truthy");
	config.insert("format".into(), output_format);
	params.insert("output_config".into(), Value::Object(config));
	Ok(())
}

/// `normalizePlatform(process.platform)` for the Rust target.
fn platform_name() -> String {
	let platform = match std::env::consts::OS {
		"macos" => "darwin",
		"windows" => "win32",
		"solaris" | "illumos" => "sunos",
		other => other,
	}
	.to_lowercase();
	if platform.contains("ios") {
		return "iOS".into();
	}
	match platform.as_str() {
		"android" => "Android".into(),
		"darwin" => "MacOS".into(),
		"win32" => "Windows".into(),
		"freebsd" => "FreeBSD".into(),
		"openbsd" => "OpenBSD".into(),
		"linux" => "Linux".into(),
		"" => "Unknown".into(),
		other => format!("Other:{other}"),
	}
}

/// `normalizeArch(process.arch)` for the Rust target, named as Node names it.
fn arch_name() -> String {
	let arch = match std::env::consts::ARCH {
		"x86" => "ia32",
		"powerpc" => "ppc",
		"powerpc64" => "ppc64",
		"loongarch64" => "loong64",
		other => other,
	};
	match arch {
		"x86_64" => "x64".into(),
		"arm" => "arm".into(),
		"aarch64" => "arm64".into(),
		"" => "unknown".into(),
		other => format!("other:{other}"),
	}
}

/// `getPlatformHeaders()`.
fn platform_headers() -> IndexMap<String, String> {
	let (os, arch, runtime, runtime_version) = if cfg!(target_arch = "wasm32") {
		("Unknown".into(), "unknown".into(), "unknown", "unknown".to_string())
	} else {
		(
			platform_name(),
			arch_name(),
			"rust",
			env!("CARGO_PKG_RUST_VERSION").to_string(),
		)
	};
	IndexMap::from([
		("X-Stainless-Lang".to_string(), "js".to_string()),
		("X-Stainless-Package-Version".to_string(), VERSION.to_string()),
		("X-Stainless-OS".to_string(), os),
		("X-Stainless-Arch".to_string(), arch),
		("X-Stainless-Runtime".to_string(), runtime.to_string()),
		("X-Stainless-Runtime-Version".to_string(), runtime_version),
	])
}

/// `AccessToken`.
#[derive(Clone, Debug, PartialEq)]
pub struct AccessToken {
	pub token: String,
	/// Unix epoch seconds. `None` means no expiry (cache forever).
	pub expires_at: Option<f64>,
}

/// `AccessTokenProvider`; the argument is `opts.forceRefresh`.
pub type AccessTokenProvider = Rc<dyn Fn(bool) -> LocalBoxFuture<'static, PiResult<AccessToken>>>;

/// `IdentityTokenProvider`.
pub type IdentityTokenProvider = Rc<dyn Fn() -> LocalBoxFuture<'static, PiResult<String>>>;

type PendingRefresh = Shared<LocalBoxFuture<'static, PiResult<AccessToken>>>;

#[derive(Default)]
struct TokenCacheState {
	cached: Option<AccessToken>,
	pending_refresh: Option<PendingRefresh>,
	next_force: bool,
	last_advisory_error: f64,
}

/// Wraps an [`AccessTokenProvider`] with two-tier proactive refresh
/// and concurrent deduplication.
///
/// Refresh policy on each [`TokenCache::get_token`] call:
///
/// - No cached token → call provider (blocking), cache, return.
/// - Cached with `expiresAt == null` → return cached forever.
/// - More than 120s remaining → return cached.
/// - 30–120s remaining (advisory window) → return stale token immediately,
///   kick off background refresh. On failure, log and keep stale.
/// - Less than 30s remaining or expired (mandatory) → block and refresh.
///   On failure, throw.
///
/// Concurrent mandatory callers coalesce into a single provider call.
pub struct TokenCache {
	provider: AccessTokenProvider,
	state: Rc<RefCell<TokenCacheState>>,
}

impl TokenCache {
	pub fn new(provider: AccessTokenProvider) -> Self {
		Self {
			provider,
			state: Rc::default(),
		}
	}

	pub async fn get_token(&self) -> PiResult<String> {
		let (force, cached) = {
			let mut state = self.state.borrow_mut();
			let force = state.next_force;
			state.next_force = false;
			(force, state.cached.clone())
		};

		let Some(cached) = cached.filter(|_| !force) else {
			return Ok(self.refresh(force).await?.token);
		};

		let Some(expires_at) = cached.expires_at else {
			return Ok(cached.token);
		};

		let remaining = expires_at - now_as_seconds();

		if remaining > ADVISORY_REFRESH_THRESHOLD_IN_SECONDS {
			return Ok(cached.token);
		}

		if remaining > MANDATORY_REFRESH_THRESHOLD_IN_SECONDS {
			self.background_refresh();
			return Ok(cached.token);
		}

		Ok(self.refresh(false).await?.token)
	}

	/// Clears the cached token and marks the next [`TokenCache::get_token`] as a forced
	/// refresh, so the underlying provider bypasses any on-disk freshness check.
	/// Called after a 401 — the server has just told us the token is bad even
	/// if its `expires_at` still looks fresh.
	pub fn invalidate(&self) {
		let mut state = self.state.borrow_mut();
		state.cached = None;
		state.next_force = true;
	}

	/// Mandatory refresh. Joins any in-flight refresh unless forced — a forced
	/// refresh must not coalesce into a non-forced one that may re-serve the
	/// same stale disk token.
	fn refresh(&self, force: bool) -> PendingRefresh {
		if !force && let Some(pending) = self.state.borrow().pending_refresh.clone() {
			return pending;
		}
		self.do_refresh(force)
	}

	/// Advisory background refresh. Shares the same in-flight promise as
	/// mandatory refreshes for deduplication, but swallows errors so the
	/// stale cached token keeps being served. Backs off for
	/// [`ADVISORY_REFRESH_BACKOFF_IN_SECONDS`] after a failure so an
	/// outage during the advisory window doesn't hammer the token endpoint.
	fn background_refresh(&self) {
		{
			let state = self.state.borrow();
			if state.pending_refresh.is_some() {
				return;
			}
			if now_as_seconds() - state.last_advisory_error < ADVISORY_REFRESH_BACKOFF_IN_SECONDS {
				return;
			}
		}
		let refresh = self.do_refresh(false);
		let state = self.state.clone();
		pi_host::spawn_local(async move {
			if refresh.await.is_err() {
				// Advisory failure: keep serving the stale cached token.
				state.borrow_mut().last_advisory_error = now_as_seconds();
			}
		});
	}

	/// Core refresh. Sets the pending refresh so concurrent callers
	/// (both advisory and mandatory) coalesce into a single provider call.
	fn do_refresh(&self, force: bool) -> PendingRefresh {
		let (sender, receiver) = oneshot::channel();
		let request = (self.provider)(force);
		let state = self.state.clone();
		pi_host::spawn_local(async move {
			let result = request.await;
			{
				let mut state = state.borrow_mut();
				if let Ok(token) = &result {
					state.cached = Some(token.clone());
				}
				state.pending_refresh = None;
			}
			let _ = sender.send(result);
		});
		let pending: PendingRefresh = async move {
			match receiver.await {
				Ok(result) => result,
				Err(_) => futures::future::pending().await,
			}
		}
		.boxed_local()
		.shared();
		self.state.borrow_mut().pending_refresh = Some(pending.clone());
		pending
	}
}

/// Reads a JWT from a file on every call. Supports automatic rotation
/// (e.g. Kubernetes projected service-account tokens).
pub fn identity_token_from_file(path: &str) -> PiResult<IdentityTokenProvider> {
	if path.is_empty() {
		return Err(anthropic_error("Identity token file path is empty"));
	}
	let path = path.to_string();
	Ok(Rc::new(move || {
		let path = path.clone();
		Box::pin(async move {
			let content = pi_host::read_file(&path).await.map_err(|error| {
				anthropic_error(format!(
					"Failed to read identity token file at {path}: {}",
					error_string(&error)
				))
			})?;
			let token = js_trim(&content);
			if token.is_empty() {
				return Err(anthropic_error(format!("Identity token file at {path} is empty")));
			}
			Ok(token.to_string())
		})
	}))
}

/// Wraps a static JWT string as an [`IdentityTokenProvider`].
pub fn identity_token_from_value(token: &str) -> PiResult<IdentityTokenProvider> {
	if token.is_empty() {
		return Err(anthropic_error("Identity token value is empty"));
	}
	let token = token.to_string();
	Ok(Rc::new(move || {
		let token = token.clone();
		Box::pin(async move { Ok(token) })
	}))
}

/// Rejects base URLs that would cause a JWT assertion or refresh token to be
/// sent over cleartext HTTP. Loopback hosts are allowed for local development.
pub fn require_secure_token_endpoint(base_url: &str) -> PiResult<()> {
	if base_url.is_empty() {
		return Ok(());
	}
	let url = url::Url::parse(base_url).map_err(|_| {
		workload_identity_error(
			format!("Invalid token endpoint base URL \"{base_url}\": TypeError: Invalid URL"),
			None,
			Value::Null,
			None,
		)
	})?;
	if url.scheme() == "https" {
		return Ok(());
	}
	let host = url.host_str().unwrap_or("").to_lowercase();
	let host = host.trim_start_matches('[').trim_end_matches(']');
	if url.scheme() == "http" && (host == "localhost" || host == "127.0.0.1" || host == "::1") {
		return Ok(());
	}
	Err(workload_identity_error(
		format!("Refusing to send credential over non-https token endpoint \"{base_url}\""),
		None,
		Value::Null,
		None,
	))
}

/// Returns a redacted copy of a token-endpoint error body for safe inclusion
/// in an exception. Strings are truncated; objects keep only the RFC 6749
/// `5.2 error fields.
pub fn redact_sensitive(body: &Value) -> Value {
	match body {
		Value::Null => Value::Null,
		Value::String(text) => match json_parse(text) {
			Ok(parsed) => Value::String(json_stringify(&redact_sensitive(&parsed))),
			Err(_) => {
				let length = utf16_len(text);
				if length <= MAX_ERROR_BODY_CHARS {
					return body.clone();
				}
				Value::String(format!(
					"{}... <{} more chars>",
					utf16_prefix(text, MAX_ERROR_BODY_CHARS),
					length - MAX_ERROR_BODY_CHARS
				))
			}
		},
		Value::Object(body) => Value::Object(
			body.iter()
				.filter(|(key, _)| matches!(key.as_str(), "error" | "error_description" | "error_uri"))
				.map(|(key, value)| (key.clone(), value.clone()))
				.collect(),
		),
		_ => Value::Null,
	}
}

/// `String(value)` of a JSON primitive, as a template literal prints it.
pub(crate) fn template_string(value: &Value) -> String {
	match value {
		Value::String(text) => text.clone(),
		Value::Number(number) => number_to_string(number.as_f64().unwrap_or(f64::NAN)),
		Value::Bool(value) => value.to_string(),
		Value::Null => "null".into(),
		Value::Array(items) => items
			.iter()
			.map(|item| {
				if item.is_null() {
					String::new()
				} else {
					template_string(item)
				}
			})
			.collect::<Vec<_>>()
			.join(","),
		Value::Object(_) => "[object Object]".into(),
	}
}

/// `Number(value)` of a JSON value.
fn js_number(value: Option<&Value>) -> f64 {
	match value {
		None => f64::NAN,
		Some(Value::Null) => 0.0,
		Some(Value::Bool(value)) => f64::from(u8::from(*value)),
		Some(Value::Number(number)) => number.as_f64().unwrap_or(f64::NAN),
		Some(Value::String(text)) => pi_host::js::string_to_number(text),
		Some(value @ Value::Array(_)) => pi_host::js::string_to_number(&template_string(value)),
		Some(Value::Object(_)) => f64::NAN,
	}
}

/// Reads at most [`MAX_TOKEN_RESPONSE_BYTES`] of a response body as UTF-8 text.
async fn read_limited_text(response: HttpResponse) -> PiResult<String> {
	use futures::StreamExt;
	let mut body = response.body;
	let mut merged: Vec<u8> = Vec::new();
	while let Some(chunk) = body.next().await {
		let chunk = chunk?;
		if merged.len() + chunk.len() > MAX_TOKEN_RESPONSE_BYTES {
			let remaining = MAX_TOKEN_RESPONSE_BYTES - merged.len();
			merged.extend_from_slice(&chunk[..remaining]);
			break;
		}
		merged.extend_from_slice(&chunk);
	}
	let merged = merged.strip_prefix(&[0xEF, 0xBB, 0xBF]).unwrap_or(&merged);
	Ok(String::from_utf8_lossy(merged).into_owned())
}

/// Reads the response body as text, parses it as a token-endpoint JSON
/// response, validates `access_token` is present, and rejects a non-Bearer
/// `token_type` when one is provided.
async fn parse_token_response(response: HttpResponse, request_id: Option<&str>) -> PiResult<Map<String, Value>> {
	let status = response.status;
	let text = read_limited_text(response).await?;
	let data = json_parse(&text).map_err(|_| {
		workload_identity_error(
			format!("Token endpoint returned non-JSON response (status {status})"),
			Some(status),
			redact_sensitive(&Value::String(text.clone())),
			request_id,
		)
	})?;
	if matches!(data, Value::Null) {
		return Err(PiError::type_error(
			"Cannot read properties of null (reading 'access_token')",
		));
	}
	if !data.get("access_token").is_some_and(truthy) {
		return Err(workload_identity_error(
			format!(
				"Token endpoint response missing access_token: {}",
				json_stringify(&redact_sensitive(&data))
			),
			Some(status),
			redact_sensitive(&data),
			request_id,
		));
	}
	match data.get("token_type") {
		Some(Value::String(token_type)) if !token_type.is_empty() && token_type.to_lowercase() != "bearer" => {
			return Err(workload_identity_error(
				format!("Token endpoint response: unsupported token_type \"{token_type}\" (want Bearer)"),
				Some(status),
				redact_sensitive(&data),
				request_id,
			));
		}
		Some(token_type) if !token_type.is_string() && truthy(token_type) => {
			return Err(PiError::type_error("data.token_type.toLowerCase is not a function"));
		}
		_ => {}
	}
	Ok(match data {
		Value::Object(data) => data,
		_ => Map::new(),
	})
}

/// `OIDCFederationConfig`.
#[derive(Clone)]
pub struct OidcFederationConfig {
	pub identity_token_provider: IdentityTokenProvider,
	pub federation_rule_id: String,
	pub organization_id: String,
	pub service_account_id: Option<String>,
	pub workspace_id: Option<String>,
	pub base_url: String,
	pub fetch: Option<FetchFunction>,
	/// Overrides the outgoing User-Agent header on the token exchange.
	pub user_agent: Option<String>,
}

/// Exchanges an external OIDC JWT for an Anthropic access token via the
/// RFC 7523 jwt-bearer grant.
///
/// Each invocation performs a fresh token exchange. Wrap in a
/// [`TokenCache`] to avoid exchanging on every request.
pub fn oidc_federation_provider(config: OidcFederationConfig) -> AccessTokenProvider {
	Rc::new(move |_force| {
		let config = config.clone();
		Box::pin(async move {
			require_secure_token_endpoint(&config.base_url)?;

			let jwt = (config.identity_token_provider)().await?;
			// The token endpoint enforces a 16 KiB assertion limit; surface a clear
			// client-side error so misconfigured projected-token sources are
			// diagnosable without a server round-trip.
			let jwt_length = utf16_len(&jwt);
			if jwt_length > 16 * 1024 {
				return Err(workload_identity_error(
					format!(
						"Identity token is {} KiB, exceeds the 16 KiB assertion limit",
						number_to_string((jwt_length as f64 / 1024.0).ceil())
					),
					None,
					Value::Null,
					None,
				));
			}

			let mut body = Map::new();
			body.insert("grant_type".into(), GRANT_TYPE_JWT_BEARER.into());
			body.insert("assertion".into(), jwt.into());
			body.insert("federation_rule_id".into(), config.federation_rule_id.clone().into());
			body.insert("organization_id".into(), config.organization_id.clone().into());
			if let Some(service_account_id) = config.service_account_id.as_ref().filter(|id| !id.is_empty()) {
				body.insert("service_account_id".into(), service_account_id.clone().into());
			}
			if let Some(workspace_id) = config.workspace_id.as_ref().filter(|id| !id.is_empty()) {
				body.insert("workspace_id".into(), workspace_id.clone().into());
			}

			let url = format!("{}{TOKEN_ENDPOINT}", config.base_url);
			let mut request = HttpRequest::new("POST", url.clone());
			request.headers.insert("Content-Type".into(), "application/json".into());
			request.headers.insert(
				"anthropic-beta".into(),
				format!("{OAUTH_API_BETA_HEADER},{FEDERATION_BETA_HEADER}"),
			);
			request.headers.insert(
				"User-Agent".into(),
				config
					.user_agent
					.clone()
					.filter(|user_agent| !user_agent.is_empty())
					.unwrap_or_else(|| format!("anthropic-sdk-typescript/{VERSION} oidcFederationProvider")),
			);
			request.body = Some(json_stringify(&Value::Object(body)).into());
			let response = pi_host::fetch(request, config.fetch.as_ref()).await.map_err(|error| {
				workload_identity_error(
					format!("Failed to reach token endpoint {url}: {}", error_string(&error)),
					None,
					Value::Null,
					None,
				)
			})?;

			let request_id = response
				.header("Request-Id")
				.filter(|request_id| !request_id.is_empty())
				.map(str::to_string);

			if !response.ok() {
				let status = response.status;
				let text = response.text().await.unwrap_or_default();
				let redacted = redact_sensitive(&Value::String(text));
				// A 401 is hard to debug from the status code alone, so surface
				// guidance: check the federation rule, optionally set a workspace ID
				// (the most common fix when no workspaceId is configured), and point at
				// the Workload identity page in Claude Console for the server-side
				// authentication event log. Other statuses (5xx, 400, ...) get no hint.
				let mut hint = String::new();
				if status == 401 {
					let hint_middle = if config.workspace_id.as_ref().is_some_and(|id| !id.is_empty()) {
						""
					} else {
						"If your federation rule is scoped to multiple workspaces, set the ANTHROPIC_WORKSPACE_ID environment variable, the 'workspace_id' config key, or the `workspaceId` option. "
					};
					hint = format!(
						" Ensure your federation rule matches your identity token. {hint_middle}View your authentication events in the Workload identity page of Claude Console for more details."
					);
				}
				let request_id_text = request_id
					.as_ref()
					.map(|request_id| format!(" (request-id {request_id})"))
					.unwrap_or_default();
				return Err(workload_identity_error(
					format!(
						"Token exchange failed with status {status}{request_id_text}: {}{hint}",
						template_string(&redacted)
					),
					Some(status),
					redacted,
					request_id.as_deref(),
				));
			}

			let status = response.status;
			let data = parse_token_response(response, request_id.as_deref()).await?;
			let expires_in = js_number(data.get("expires_in"));
			if !expires_in.is_finite() {
				let redacted = redact_sensitive(&Value::Object(data));
				return Err(workload_identity_error(
					format!(
						"Token endpoint response missing required fields: {}",
						json_stringify(&redacted)
					),
					Some(status),
					redacted,
					request_id.as_deref(),
				));
			}

			Ok(AccessToken {
				token: template_string(&data["access_token"]),
				expires_at: Some(now_as_seconds() + expires_in),
			})
		})
	})
}

/// `CredentialResult`.
struct CredentialResult {
	provider: AccessTokenProvider,
	extra_headers: IndexMap<String, String>,
}

/// Resolves the identity token provider from config fields or environment variables.
///
/// Resolution order:
///   1. `identity_token.path` from the config (source: "file")
///   2. `ANTHROPIC_IDENTITY_TOKEN_FILE` env var
///   3. `ANTHROPIC_IDENTITY_TOKEN` env var (static value)
fn resolve_identity_token_provider(auth: &OidcFederationAuthentication) -> PiResult<Option<IdentityTokenProvider>> {
	if let Some(identity_token) = &auth.identity_token {
		if identity_token.source != "file" {
			return Err(workload_identity_error(
				format!(
					"identity_token.source \"{}\" is not supported by this SDK version (only \"file\")",
					identity_token.source
				),
				None,
				Value::Null,
				None,
			));
		}
		let Some(path) = identity_token.path.as_ref().filter(|path| !path.is_empty()) else {
			return Err(workload_identity_error(
				"identity_token.source \"file\" requires a non-empty path",
				None,
				Value::Null,
				None,
			));
		};
		return identity_token_from_file(path).map(Some);
	}

	if let Some(token_file) = read_env("ANTHROPIC_IDENTITY_TOKEN_FILE") {
		return identity_token_from_file(&token_file).map(Some);
	}

	if let Some(token_value) = read_env("ANTHROPIC_IDENTITY_TOKEN") {
		return identity_token_from_value(&token_value).map(Some);
	}

	Ok(None)
}

/// `resolveCredentialsFromConfig(config, options)` for an `oidc_federation` config.
fn resolve_credentials_from_config(
	config: &AnthropicConfig,
	base_url: &str,
	fetch: Option<FetchFunction>,
	user_agent: &str,
) -> PiResult<CredentialResult> {
	let effective_base_url = base_url.trim_end_matches('/').to_string();
	let auth = &config.authentication;
	if auth.r#type != "oidc_federation" {
		return Err(workload_identity_error(
			format!(
				"authentication.type \"{}\" is not a known authentication type",
				auth.r#type
			),
			None,
			Value::Null,
			None,
		));
	}
	let Some(identity_provider) = resolve_identity_token_provider(auth)? else {
		return Err(workload_identity_error(
			"oidc_federation config requires an identity token (set authentication.identity_token, ANTHROPIC_IDENTITY_TOKEN_FILE, or ANTHROPIC_IDENTITY_TOKEN)",
			None,
			Value::Null,
			None,
		));
	};
	let Some(federation_rule_id) = auth.federation_rule_id.clone().filter(|id| !id.is_empty()) else {
		return Err(workload_identity_error(
			"oidc_federation config requires 'federation_rule_id'. Set it in authentication.federation_rule_id in your profile, or via ANTHROPIC_FEDERATION_RULE_ID (profile takes precedence).",
			None,
			Value::Null,
			None,
		));
	};
	let Some(organization_id) = config.organization_id.clone().filter(|id| !id.is_empty()) else {
		return Err(workload_identity_error(
			"oidc_federation config requires organization_id (set ANTHROPIC_ORGANIZATION_ID or config.organization_id)",
			None,
			Value::Null,
			None,
		));
	};

	let provider = oidc_federation_provider(OidcFederationConfig {
		identity_token_provider: identity_provider,
		federation_rule_id,
		organization_id,
		service_account_id: auth.service_account_id.clone(),
		workspace_id: config.workspace_id.clone(),
		base_url: effective_base_url,
		fetch,
		user_agent: Some(user_agent.to_string()),
	});

	// For federation profiles workspace_id is sent in the jwt-bearer exchange
	// body, not as a request header (the minted token is already
	// workspace-scoped, so the header would be ignored).
	Ok(CredentialResult {
		provider,
		extra_headers: IndexMap::new(),
	})
}

/// The part of the SDK's `Anthropic` client that `api/anthropic-messages.ts` calls:
/// `client.beta.messages.create(params, options).asResponse()` for a streaming request.
///
/// An error for a failed request must carry `data.headers` (an object, or `null` when absent) and `data.type` like
/// [`AnthropicClient`]'s: that key is how the caller tells an SDK `APIError` from other failures
/// (the TS `instanceof Anthropic.APIError`), for example to fall back from strict tools.
pub trait AnthropicClientApi {
	/// The raw response of the streaming request.
	fn beta_messages_create<'a>(
		&'a self,
		params: &'a Value,
		options: RequestOptions,
	) -> LocalBoxFuture<'a, PiResult<HttpResponse>>;
}

impl AnthropicClientApi for AnthropicClient {
	fn beta_messages_create<'a>(
		&'a self,
		params: &'a Value,
		options: RequestOptions,
	) -> LocalBoxFuture<'a, PiResult<HttpResponse>> {
		Box::pin(AnthropicClient::beta_messages_create(self, params, options))
	}
}

/// Shared auth state. A [`AnthropicClient::with_default_headers`] clone receives the parent's instance.
#[derive(Default)]
struct AuthState {
	token_cache: Option<TokenCache>,
	extra_headers: IndexMap<String, String>,
}

/// Per-request auth flags (`RequestAuthFlags`).
#[derive(Default)]
struct RequestAuthFlags {
	used_token_cache: Cell<bool>,
	did_refresh_for_401: Cell<bool>,
}

/// The Anthropic API client (`BaseAnthropic` / `Anthropic`).
pub struct AnthropicClient {
	options: ClientOptions,
	base_url: String,
	timeout: f64,
	max_retries: f64,
	api_key: Option<String>,
	auth_token: Option<String>,
	auth_state: Rc<AuthState>,
}

const CREDENTIAL_OPTIONS: [&str; 5] = ["apiKey", "authToken", "credentials", "config", "profile"];

impl AnthropicClient {
	/// `new Anthropic(options)`.
	pub fn new(options: ClientOptions) -> PiResult<Self> {
		Self::construct(options, None)
	}

	fn construct(mut options: ClientOptions, inherited: Option<Rc<AuthState>>) -> PiResult<Self> {
		let passed: Vec<&str> = [
			("apiKey", options.api_key.is_some()),
			("authToken", options.auth_token.is_some()),
			("config", options.config.is_some()),
		]
		.into_iter()
		.filter(|(_, passed)| *passed)
		.map(|(name, _)| name)
		.collect();
		if passed.len() > 1 {
			let list = |names: &[&str]| {
				names
					.iter()
					.map(|name| format!("`{name}`"))
					.collect::<Vec<_>>()
					.join(", ")
			};
			return Err(PiError::type_error(format!(
				"Pass at most one of {}. Received {}.",
				list(&CREDENTIAL_OPTIONS),
				list(&passed)
			)));
		}

		let base_url = options
			.base_url
			.clone()
			.filter(|base_url| !base_url.is_empty())
			.unwrap_or_else(|| DEFAULT_BASE_URL.to_string());
		let timeout = options.timeout.unwrap_or(DEFAULT_TIMEOUT);
		let max_retries = validate_positive_integer("maxRetries", options.max_retries.unwrap_or(2.0))?;

		if let Some(custom_headers_env) = read_env("ANTHROPIC_CUSTOM_HEADERS") {
			let mut parsed: ProviderHeaders = IndexMap::new();
			for line in custom_headers_env.split('\n') {
				if let Some(colon) = line.find(':') {
					parsed.insert(
						js_trim(&line[..colon]).to_string(),
						Some(js_trim(&line[colon + 1..]).to_string()),
					);
				}
			}
			for (name, value) in options.default_headers.take().unwrap_or_default() {
				parsed.insert(name, value);
			}
			options.default_headers = Some(parsed);
		}

		let api_key = options.api_key.clone();
		let auth_token = options.auth_token.clone();

		let auth_state = match inherited {
			Some(inherited) => inherited,
			None => {
				let mut state = AuthState::default();
				if let Some(config) = &options.config {
					let result = resolve_credentials_from_config(
						config,
						&base_url,
						options.fetch.clone(),
						&format!("Anthropic/JS {VERSION}"),
					)?;
					state.token_cache = Some(TokenCache::new(result.provider));
					state.extra_headers = result.extra_headers;
				}
				Rc::new(state)
			}
		};

		Ok(Self {
			options,
			base_url,
			timeout,
			max_retries,
			api_key,
			auth_token,
			auth_state,
		})
	}

	/// `client.withOptions({ defaultHeaders })`: a client with the same options, credential and shared auth state
	/// (token cache) whose default headers are replaced.
	pub fn with_default_headers(&self, default_headers: ProviderHeaders) -> PiResult<Self> {
		let options = ClientOptions {
			base_url: Some(self.base_url.clone()),
			max_retries: Some(self.max_retries),
			timeout: Some(self.timeout),
			default_headers: Some(default_headers),
			..self.options.clone()
		};
		Self::construct(options, Some(self.auth_state.clone()))
	}

	/// The client's base URL.
	pub fn base_url(&self) -> &str {
		&self.base_url
	}

	/// `client.beta.messages.create(params, options).asResponse()` for a streaming request: the raw response of
	/// `POST /v1/messages?beta=true`. `betas`, `user_profile_id` and `workspace_id` become headers instead of body
	/// fields.
	pub async fn beta_messages_create(&self, params: &Value, options: RequestOptions) -> PiResult<HttpResponse> {
		let mut body = params.as_object().cloned().unwrap_or_default();
		transform_output_format(&mut body)?;
		let betas = body.shift_remove("betas");
		let user_profile_id = body.shift_remove("user_profile_id");
		let workspace_id = body.shift_remove("workspace_id");

		let timeout = options.timeout.or(self.options.timeout).unwrap_or(DEFAULT_TIMEOUT);

		let mut create_headers = Vec::new();
		if let Some(betas) = betas.as_ref().filter(|betas| !betas.is_null()) {
			create_headers.push(("anthropic-beta".into(), Some(template_string(betas))));
		}
		if let Some(user_profile_id) = user_profile_id.as_ref().filter(|id| !id.is_null()) {
			create_headers.push((
				"anthropic-user-profile-id".into(),
				Some(template_string(user_profile_id)),
			));
		}
		if let Some(workspace_id) = workspace_id.as_ref().filter(|id| !id.is_null()) {
			create_headers.push(("anthropic-workspace-id".into(), Some(template_string(workspace_id))));
		}
		let headers = build_headers(vec![
			HeaderSource::record(create_headers),
			nullable_source(options.headers.as_ref()),
		]);

		self.make_request(
			"post",
			"/v1/messages?beta=true",
			Value::Object(body),
			RequestOptions {
				timeout: Some(timeout),
				..options
			},
			headers,
		)
		.await
	}

	/// `makeRequest` with its `retryRequest` recursion as a loop.
	async fn make_request(
		&self,
		method: &str,
		path: &str,
		body: Value,
		options: RequestOptions,
		headers: NullableHeaders,
	) -> PiResult<HttpResponse> {
		let max_retries = validate_positive_integer("maxRetries", options.max_retries.unwrap_or(self.max_retries))?;
		let mut retries_remaining = max_retries;
		let flags = RequestAuthFlags::default();

		loop {
			let (mut request, timeout) = self
				.build_request(
					method,
					path,
					&body,
					&options,
					&headers,
					max_retries - retries_remaining,
					&flags,
				)
				.await?;

			if options.signal.as_ref().is_some_and(AbortSignal::aborted) {
				return Err(user_abort_error());
			}

			let controller = AbortController::new();
			let mut listener = options.signal.as_ref().map(|signal| {
				let controller = controller.clone();
				(signal.clone(), signal.add_listener(move |_| controller.abort()))
			});
			// `releaseRequestSignal(controller)`: a failed or retried attempt stops listening to the caller's signal.
			let mut release = move || {
				if let Some((signal, id)) = listener.take() {
					signal.remove_listener(id);
				}
			};
			request.signal = Some(controller.signal());
			let response = self.fetch_with_timeout(request, timeout, &controller).await;

			let response = match response {
				Err(error) => {
					release();
					if options.signal.as_ref().is_some_and(AbortSignal::aborted) {
						return Err(user_abort_error());
					}
					// detect native connection timeout errors
					let cause = error
						.data
						.as_ref()
						.and_then(|data| data.get("cause"))
						.map(template_string)
						.unwrap_or_default();
					let is_timeout = error.is_abort()
						|| error.message.contains("FetchRequestCanceledException")
						|| timed_out_pattern(&format!("{}{cause}", error_string(&error)));
					if retries_remaining > 0.0 {
						let delay = self.calculate_default_retry_timeout_millis(
							retries_remaining,
							options.max_retries.unwrap_or(self.max_retries),
						);
						sdk_sleep(delay, options.signal.as_ref()).await;
						retries_remaining -= 1.0;
						continue;
					}
					if is_timeout {
						return Err(connection_timeout_error());
					}
					return Err(connection_error(&error));
				}
				Ok(response) => response,
			};

			if !response.ok() {
				release();
				let should_retry = self.should_retry(&response, &flags);
				if retries_remaining > 0.0 && should_retry {
					let delay = self.retry_delay(&response.headers, retries_remaining, &options);
					drop(response);
					sdk_sleep(delay, options.signal.as_ref()).await;
					retries_remaining -= 1.0;
					continue;
				}

				let status = response.status;
				let response_headers = response.headers.clone();
				let err_text = response.text().await.unwrap_or_else(|error| error.message);
				let err_json = json_parse(&err_text).ok();
				let err_message = if err_json.as_ref().is_some_and(truthy) {
					None
				} else {
					Some(err_text.as_str())
				};
				return Err(make_status_error(status, err_json, err_message, &response_headers));
			}

			return Ok(response);
		}
	}

	/// `fetchWithTimeout`: the request timeout covers the time until the response headers arrive.
	async fn fetch_with_timeout(
		&self,
		request: HttpRequest,
		ms: f64,
		controller: &AbortController,
	) -> PiResult<HttpResponse> {
		let request = self.prepare_request(request);
		let fetch = pi_host::fetch(request, self.options.fetch.as_ref());
		let timer = pi_host::sleep(ms, None);
		futures::pin_mut!(fetch, timer);
		match futures::future::select(fetch, timer).await {
			futures::future::Either::Left((response, _)) => response,
			futures::future::Either::Right(_) => {
				controller.abort();
				Err(PiError::abort())
			}
		}
	}

	/// `shouldRetry(response, options)`.
	fn should_retry(&self, response: &HttpResponse, flags: &RequestAuthFlags) -> bool {
		// Reactive refresh: on a 401 from a request that used the token cache,
		// invalidate and retry once. Only fires when this specific request was
		// bearer-authenticated (not when an apiKey was used) and only once per
		// request — a second 401 after refresh falls through to the normal
		// retry policy below (which treats 4xx as non-retryable).
		if response.status == 401
			&& let Some(token_cache) = &self.auth_state.token_cache
			&& flags.used_token_cache.get()
			&& !flags.did_refresh_for_401.get()
		{
			flags.did_refresh_for_401.set(true);
			token_cache.invalidate();
			return true;
		}

		// Note this is not a standard header.
		match response.header("x-should-retry") {
			Some("true") => return true,
			Some("false") => return false,
			_ => {}
		}

		matches!(response.status, 408 | 409 | 429) || response.status >= 500
	}

	/// The delay `retryRequest` sleeps before a retry.
	fn retry_delay(&self, headers: &IndexMap<String, String>, retries_remaining: f64, options: &RequestOptions) -> f64 {
		let header = |name: &str| {
			headers
				.iter()
				.find(|(key, _)| key.eq_ignore_ascii_case(name))
				.map(|(_, value)| value.as_str())
		};
		let mut timeout_millis: Option<f64> = None;

		// Note the `retry-after-ms` header may not be standard, but is a good idea and we'd like proactive support for it.
		if let Some(retry_after_millis) = header("retry-after-ms").filter(|value| !value.is_empty()) {
			let timeout_ms = pi_host::js::parse_float(retry_after_millis);
			if !timeout_ms.is_nan() {
				timeout_millis = Some(timeout_ms);
			}
		}

		// About the Retry-After header: https://developer.mozilla.org/en-US/docs/Web/HTTP/Headers/Retry-After
		if let Some(retry_after) = header("retry-after").filter(|value| !value.is_empty())
			&& !timeout_millis.is_some_and(|ms| ms != 0.0 && !ms.is_nan())
		{
			let timeout_seconds = pi_host::js::parse_float(retry_after);
			timeout_millis = Some(if timeout_seconds.is_nan() {
				pi_host::js::date_parse(retry_after) - pi_host::now_ms()
			} else {
				timeout_seconds * 1000.0
			});
		}

		// If the API asks us to wait a certain amount of time, do what it says, as long as it's a positive delay that
		// one timer can represent. Otherwise calculate a default.
		match timeout_millis {
			Some(ms) if ms > 0.0 && ms <= 2_147_483_647.0 => ms,
			_ => self.calculate_default_retry_timeout_millis(
				retries_remaining,
				options.max_retries.unwrap_or(self.max_retries),
			),
		}
	}

	fn calculate_default_retry_timeout_millis(&self, retries_remaining: f64, max_retries: f64) -> f64 {
		let initial_retry_delay = 0.5;
		let max_retry_delay = 8.0;

		let num_retries = max_retries - retries_remaining;

		// Apply exponential backoff, but not more than the max.
		let sleep_seconds = f64::min(initial_retry_delay * 2f64.powf(num_retries), max_retry_delay);

		// Apply some jitter, take up to at most 25 percent of the retry time.
		let jitter = 1.0 - rand::random::<f64>() * 0.25;

		sleep_seconds * jitter * 1000.0
	}

	/// `buildURL(path)`.
	fn build_url(&self, path: &str) -> PiResult<String> {
		let base_url = &self.base_url;
		let joined = if base_url.ends_with('/') && path.starts_with('/') {
			format!("{base_url}{}", &path[1..])
		} else {
			format!("{base_url}{path}")
		};
		let mut url = url::Url::parse(&joined).map_err(|_| PiError::type_error("Invalid URL"))?;
		// `Object.fromEntries(url.searchParams)` re-encoded by `qs.stringify` (RFC 3986).
		let path_query: IndexMap<String, String> = url.query_pairs().into_owned().collect();
		if !path_query.is_empty() {
			let query: Vec<String> = path_query
				.iter()
				.map(|(key, value)| format!("{}={}", qs_encode(key), qs_encode(value)))
				.collect();
			url.set_query(Some(&query.join("&")));
		}
		Ok(url.to_string())
	}

	/// `buildRequest(options, { retryCount })`.
	#[allow(clippy::too_many_arguments)]
	async fn build_request(
		&self,
		method: &str,
		path: &str,
		body: &Value,
		options: &RequestOptions,
		headers: &NullableHeaders,
		retry_count: f64,
		flags: &RequestAuthFlags,
	) -> PiResult<(HttpRequest, f64)> {
		let url = self.build_url(path)?;
		let timeout = validate_positive_integer("timeout", options.timeout.unwrap_or(self.timeout))?;

		let mut defaults = IndexMap::from([
			("Accept".to_string(), "application/json".to_string()),
			("User-Agent".to_string(), format!("Anthropic/JS {VERSION}")),
			("X-Stainless-Retry-Count".to_string(), number_to_string(retry_count)),
			(
				"X-Stainless-Timeout".to_string(),
				number_to_string((timeout / 1000.0).trunc()),
			),
		]);
		defaults.extend(platform_headers());
		if self.options.dangerously_allow_browser == Some(true) {
			defaults.insert("anthropic-dangerous-direct-browser-access".into(), "true".into());
		}
		defaults.insert("anthropic-version".into(), "2026-10-21".into());

		let request_headers = build_headers(vec![
			record_source(&defaults),
			self.auth_headers(flags).await?,
			nullable_source(self.options.default_headers.as_ref()),
			HeaderSource::record(vec![("content-type".into(), Some("application/json".into()))]),
			headers.as_source(),
		]);

		self.validate_headers(&request_headers)?;

		let mut request = HttpRequest::new(method.to_ascii_uppercase(), url);
		request.headers = request_headers.entries();
		request.body = Some(json_stringify(body).into());
		Ok((request, timeout))
	}

	/// `authHeaders(opts)`.
	async fn auth_headers(&self, flags: &RequestAuthFlags) -> PiResult<HeaderSource> {
		// If we have a token cache and no API key is set, use token auth
		if let Some(token_cache) = &self.auth_state.token_cache
			&& self.api_key.is_none()
		{
			let token = token_cache.get_token().await?;
			flags.used_token_cache.set(true);
			return Ok(build_headers(vec![HeaderSource::record(vec![(
				"Authorization".into(),
				Some(format!("Bearer {token}")),
			)])])
			.as_source());
		}
		let bearer = self
			.auth_token
			.as_ref()
			.map(|auth_token| vec![("Authorization".to_string(), Some(format!("Bearer {auth_token}")))])
			.unwrap_or_default();
		let bearer = HeaderSource::record(bearer);
		let api_key = self
			.api_key
			.as_ref()
			.map(|api_key| vec![("Authorization".to_string(), Some(format!("Bearer {api_key}")))])
			.unwrap_or_default();
		let api_key = HeaderSource::record(api_key);
		Ok(build_headers(vec![bearer, api_key]).as_source())
	}

	/// `validateHeaders({ values, nulls })`.
	fn validate_headers(&self, headers: &NullableHeaders) -> PiResult<()> {
		let present = |name: &str| headers.get(name).is_some_and(|value| !value.is_empty());
		if present("x-api-key") || present("authorization") {
			return Ok(());
		}
		if self.auth_state.token_cache.is_some() {
			return Ok(()); // auth will be injected per-request via authHeaders
		}
		if headers.nulls.contains("authorization") {
			return Ok(());
		}

		Err(PiError::new(
			"Could not resolve authentication method. Expected one of apiKey, authToken, credentials, config, or profile to be set. Or for the \"Authorization\" header to be explicitly omitted",
		))
	}

	/// `prepareRequest(request)`: token-authenticated requests carry the credential's extra headers and the OAuth
	/// beta, appended to any existing `anthropic-beta` value.
	fn prepare_request(&self, mut request: HttpRequest) -> HttpRequest {
		if self.auth_state.token_cache.is_some() && self.api_key.is_none() {
			for (name, value) in &self.auth_state.extra_headers {
				if request.header(name).is_none() {
					request.headers.insert(name.to_ascii_lowercase(), value.clone());
				}
			}
			let existing: Option<Vec<String>> = request
				.header("anthropic-beta")
				.map(|value| value.split(',').map(|beta| beta.trim().to_string()).collect());
			if !existing
				.as_ref()
				.is_some_and(|existing| existing.iter().any(|beta| beta == OAUTH_API_BETA_HEADER))
			{
				let mut betas = existing.unwrap_or_default();
				betas.push(OAUTH_API_BETA_HEADER.to_string());
				request.headers.insert("anthropic-beta".into(), betas.join(","));
				request.headers.sort_keys();
			}
		}
		request
	}
}

/// `/timed? ?out/i.test(text)`.
fn timed_out_pattern(text: &str) -> bool {
	let lower = text.to_ascii_lowercase();
	["timed out", "timedout", "time out", "timeout"]
		.iter()
		.any(|pattern| lower.contains(pattern))
}

/// The SDK's `sleep(ms, signal)`: resolves after `ms`, or early when `signal` aborts.
async fn sdk_sleep(ms: f64, signal: Option<&AbortSignal>) {
	let _ = pi_host::sleep(ms, signal).await;
}

#[cfg(test)]
mod tests {
	use super::*;

	fn record(entries: &[(&str, &str)]) -> HeaderSource {
		HeaderSource::record(
			entries
				.iter()
				.map(|(name, value)| (name.to_string(), Some(value.to_string())))
				.collect(),
		)
	}

	#[test]
	fn record_keys_replace_and_built_sources_append() {
		let headers = build_headers(vec![
			record(&[("accept", "application/json")]),
			record(&[("Accept", "text/plain")]),
		]);
		assert_eq!(headers.get("accept").as_deref(), Some("text/plain"));
		let headers = build_headers(vec![record(&[("X-A", "1"), ("x-a", "2")])]);
		assert_eq!(headers.get("x-a").as_deref(), Some("2"));
		let built = HeaderSource {
			record: false,
			entries: vec![("x-a".into(), Some("1".into())), ("x-a".into(), Some("2".into()))],
		};
		assert_eq!(build_headers(vec![built]).get("x-a").as_deref(), Some("1, 2"));
	}

	#[test]
	fn moves_output_format_into_output_config() {
		let mut params = json!({ "model": "m", "output_format": { "type": "json_schema" }, "max_tokens": 1 })
			.as_object()
			.unwrap()
			.clone();
		transform_output_format(&mut params).unwrap();
		assert_eq!(
			json_stringify(&Value::Object(params)),
			r#"{"model":"m","max_tokens":1,"output_config":{"format":{"type":"json_schema"}}}"#
		);
		let mut both = json!({ "output_format": { "type": "x" }, "output_config": { "format": { "type": "y" } } })
			.as_object()
			.unwrap()
			.clone();
		assert!(
			transform_output_format(&mut both)
				.unwrap_err()
				.message
				.starts_with("Both output_format")
		);
	}

	#[test]
	fn re_encodes_the_query_of_a_base_url_with_a_query() {
		pi_host::testing::run(|_| async {
			let client = AnthropicClient::new(ClientOptions {
				api_key: Some("key".into()),
				base_url: Some("https://proxy.test/x?k=v".into()),
				..ClientOptions::default()
			})
			.unwrap();
			assert_eq!(
				client.build_url("/v1/messages?beta=true").unwrap(),
				"https://proxy.test/x?k=v%2Fv1%2Fmessages%3Fbeta%3Dtrue"
			);
		});
	}

	#[test]
	fn trims_like_javascript() {
		assert_eq!(js_trim("\u{feff} token\n"), "token");
		assert_eq!(js_trim("\u{85}token"), "\u{85}token");
	}
}
