//! Rust-only replacement for the `openai` npm SDK (version 7.19.0), limited to what pi-ai uses: a client with an API
//! key, base URL, default headers and a custom `fetch`, and `POST` requests whose body is JSON and whose response is
//! an SSE stream (`chat.completions.create({ stream: true })`, `responses.create({ stream: true })`).
//!
//! It reproduces the SDK behavior that requests and errors observe:
//! - URL building (`buildURL`): `baseURL + path` with one slash dropped, and the query of the base URL re-encoded with
//!   the SDK's `qs` encoder (RFC 3986) when present.
//! - Headers (`buildHeaders`): `Accept`, `User-Agent: OpenAI/JS 7.19.0`, `X-Stainless-Retry-Count`,
//!   `X-Stainless-Timeout`, the `X-Stainless-*` platform headers, `OpenAI-Organization`/`OpenAI-Project`
//!   (`OPENAI_ORG_ID`/`OPENAI_PROJECT_ID`), `Authorization: Bearer <apiKey>`, the default headers (after
//!   `OPENAI_CUSTOM_HEADERS`), `content-type: application/json`, then the request headers. Later sources win per
//!   lowercase name, `null` removes a header, names must be HTTP tokens, and the request carries the names lowercased
//!   and sorted, like a `Headers` object.
//! - Timeouts: each attempt aborts after `timeout` ms (default 10 minutes) until the response headers arrive; then the
//!   SSE body is read without a timeout, as in the SDK. A timeout fails with `Request timed out.`, a caller abort with
//!   `Request was aborted.`, other fetch failures with `Connection error.`.
//! - Retries (`maxRetries`, default 2): connection failures and responses the SDK retries (`x-should-retry`, 408,
//!   409, 429, >= 500), waiting `retry-after-ms`, `retry-after` (seconds or an HTTP date) when within 60 s, else
//!   exponential backoff with jitter.
//! - Errors: a non-2xx response becomes the SDK `APIError` message (`<status> <error.message | JSON>`), and an SSE
//!   `error` event or an event with an `error` field becomes an `APIError` without a status. Every error the SDK
//!   gives `status` and `headers` properties carries `data.headers` (the response headers as
//!   `Object.fromEntries(headers)`, or `null`), `PiError.status`, and `data.error` (the SDK's `error` property), as
//!   `utils/provider_retry.rs`, `utils/diagnostics.rs` and `utils/error_body.rs` expect.
//! - SSE decoding: `pi_host::sse`, `[DONE]` ends the stream, a caller abort or an `AbortError` from the body ends it
//!   quietly, and malformed event JSON fails with the SDK's `SyntaxError`.
//!
//! Differences: the `X-Stainless-OS`/`X-Stainless-Arch` headers describe the Rust build target and
//! `X-Stainless-Runtime`/`X-Stainless-Runtime-Version` are `unknown` (there is no Node.js process to describe); a
//! timeout fails at once instead of waiting for a `fetch` that ignores its signal; the undici headers-timeout message,
//! SDK logging, workload identity, X.509, admin keys, providers and non-JSON request bodies are not reproduced.

use std::rc::Rc;
use std::sync::LazyLock;

use bytes::Bytes;
use futures::StreamExt;
use futures::future::{Either, select};
use indexmap::{IndexMap, IndexSet};
use pi_host::js::{date_parse, json_stringify, number_to_string, parse_float};
use pi_host::{
	AbortController, AbortSignal, FetchFunction, HttpRequest, HttpResponse, LocalBoxStream, PiError, PiResult,
};
use regex::Regex;
use serde_json::{Map, Value};

use crate::types::ProviderHeaders;
use crate::utils::headers::headers_to_record;
use crate::utils::json_parse::json_parse;

/// Version of the replaced OpenAI SDK package.
pub const VERSION: &str = "7.19.0";
/// `OpenAI.DEFAULT_TIMEOUT`: 10 minutes.
pub const DEFAULT_TIMEOUT: f64 = 600_000.0;
const DEFAULT_MAX_RETRIES: f64 = 2.0;
const DEFAULT_BASE_URL: &str = "https://api.openai.com/v1";

/// `ClientOptions` of the SDK, limited to the options pi-ai passes.
#[derive(Clone, Default)]
pub struct ClientOptions {
	/// Defaults to `process.env.OPENAI_API_KEY`.
	pub api_key: Option<String>,
	/// Defaults to `process.env.OPENAI_ORG_ID`.
	pub organization: Option<String>,
	/// Defaults to `process.env.OPENAI_PROJECT_ID`.
	pub project: Option<String>,
	/// Defaults to `process.env.OPENAI_BASE_URL`, then `https://api.openai.com/v1`.
	pub base_url: Option<String>,
	/// Milliseconds before a request times out. Defaults to 10 minutes.
	pub timeout: Option<f64>,
	/// Retries of a failed request. Defaults to 2.
	pub max_retries: Option<f64>,
	/// Replaces the host `fetch`.
	pub fetch: Option<FetchFunction>,
	/// Headers of every request; a `None` value removes a default header.
	pub default_headers: Option<ProviderHeaders>,
	/// Query parameters of every request.
	pub default_query: Option<IndexMap<String, String>>,
	/// `AzureOpenAI`: send requests that carry an `api-key` header with `redirect: "manual"`.
	pub manual_redirect_with_api_key: bool,
}

/// `RequestOptions` of one SDK call.
#[derive(Clone, Default)]
pub struct RequestOptions {
	pub signal: Option<AbortSignal>,
	/// Milliseconds before the request times out; must be a non-negative integer.
	pub timeout: Option<f64>,
	pub max_retries: Option<f64>,
	/// Request headers; a `None` value removes a default header.
	pub headers: Option<ProviderHeaders>,
}

/// The `response` of `withResponse()`: what pi-ai reads of the `Response`.
#[derive(Clone, Debug, PartialEq)]
pub struct ApiResponse {
	pub status: u16,
	pub url: String,
	/// Lowercase names, as `Headers` iterates them.
	pub headers: IndexMap<String, String>,
}

/// `APIPromise.withResponse()`'s result.
pub struct WithResponse<T> {
	pub data: T,
	pub response: ApiResponse,
	/// `x-request-id` of the response.
	pub request_id: Option<String>,
}

/// What a create call resolves to: the SSE event stream when the body asked for `stream`, else the parsed body.
pub enum ApiData {
	Stream(LocalBoxStream<'static, PiResult<Value>>),
	Json(Value),
}

/// The SDK client (`new OpenAI(options)`).
pub struct OpenAI {
	api_key: Option<String>,
	organization: Option<String>,
	project: Option<String>,
	base_url: String,
	timeout: f64,
	max_retries: f64,
	fetch: Option<FetchFunction>,
	default_headers: DefaultHeaders,
	default_query: Option<IndexMap<String, String>>,
	manual_redirect_with_api_key: bool,
}

/// `readEnv(name)`: trimmed, and `None` when empty.
fn read_env(name: &str) -> Option<String> {
	pi_host::env(name)
		.map(|value| crate::utils::text::js_trim(&value).to_string())
		.filter(|value| !value.is_empty())
}

impl OpenAI {
	/// `new OpenAI(options)`.
	pub fn new(options: ClientOptions) -> PiResult<Self> {
		let api_key = options.api_key.or_else(|| read_env("OPENAI_API_KEY"));
		let admin_api_key = read_env("OPENAI_ADMIN_KEY");
		if api_key.as_deref().is_none_or(str::is_empty) && admin_api_key.is_none() {
			return Err(PiError::new(
				"Missing credentials. Please pass an `apiKey`, `workloadIdentity`, `adminAPIKey`, or set the `OPENAI_API_KEY` or `OPENAI_ADMIN_KEY` environment variable.",
			));
		}
		let base_url = match options.base_url {
			Some(base_url) => base_url,
			None => read_env("OPENAI_BASE_URL").unwrap_or_default(),
		};
		let raw_default_headers = options.default_headers.unwrap_or_default();
		// The SDK keeps `defaultHeaders` as given and builds them per request, so a bad name fails inside the request,
		// unless `OPENAI_CUSTOM_HEADERS` makes the constructor build them.
		let mut default_headers = DefaultHeaders::Object(raw_default_headers.clone());
		if let Some(custom_headers) = read_env("OPENAI_CUSTOM_HEADERS") {
			let mut built = NullableHeaders::default();
			let mut parsed: IndexMap<String, String> = IndexMap::new();
			for line in custom_headers.split('\n') {
				if let Some(colon) = line.find(':') {
					parsed.insert(
						crate::utils::text::js_trim(&line[..colon]).to_string(),
						crate::utils::text::js_trim(&line[colon + 1..]).to_string(),
					);
				}
			}
			for (name, value) in &parsed {
				built.set(name, Some(value))?;
			}
			for (name, value) in &raw_default_headers {
				built.set(name, value.as_deref())?;
			}
			default_headers = DefaultHeaders::Built(built);
		}
		Ok(Self {
			api_key,
			organization: options.organization.or_else(|| read_env("OPENAI_ORG_ID")),
			project: options.project.or_else(|| read_env("OPENAI_PROJECT_ID")),
			base_url: if base_url.is_empty() {
				DEFAULT_BASE_URL.to_string()
			} else {
				base_url
			},
			timeout: options.timeout.unwrap_or(DEFAULT_TIMEOUT),
			max_retries: options.max_retries.unwrap_or(DEFAULT_MAX_RETRIES),
			fetch: options.fetch,
			default_headers,
			default_query: options.default_query,
			manual_redirect_with_api_key: options.manual_redirect_with_api_key,
		})
	}

	/// `client.baseURL`.
	pub fn base_url(&self) -> &str {
		&self.base_url
	}

	/// `client.chat.completions.create(body, options).withResponse()`.
	pub async fn chat_completions_create(
		&self,
		body: &Value,
		options: &RequestOptions,
	) -> PiResult<WithResponse<ApiData>> {
		self.post_json("/chat/completions", body, options).await
	}

	/// `client.post(path, { body, ...options, stream: body.stream ?? false }).withResponse()`.
	pub async fn post_json(
		&self,
		path: &str,
		body: &Value,
		options: &RequestOptions,
	) -> PiResult<WithResponse<ApiData>> {
		let stream = body.get("stream").is_some_and(is_truthy);
		let response = self.make_request("post", path, body, options).await?;
		let api_response = ApiResponse {
			status: response.status,
			url: response.url.clone(),
			headers: response.headers.clone(),
		};
		let request_id = response.header("x-request-id").map(str::to_string);
		let data = if stream {
			ApiData::Stream(stream_from_sse_response(response, options.signal.clone()))
		} else {
			ApiData::Json(default_parse_response(response).await?)
		};
		Ok(WithResponse {
			data,
			response: api_response,
			request_id,
		})
	}

	/// `buildURL(path, query)`.
	fn build_url(&self, path: &str) -> PiResult<String> {
		let base_url = &self.base_url;
		let joined = if base_url.ends_with('/') && path.starts_with('/') {
			format!("{base_url}{}", &path[1..])
		} else {
			format!("{base_url}{path}")
		};
		let mut url = url::Url::parse(&joined).map_err(|_| PiError::type_error("Invalid URL"))?;
		let mut query: IndexMap<String, String> = IndexMap::new();
		for (key, value) in url.query_pairs() {
			query.insert(key.into_owned(), value.into_owned());
		}
		if let Some(default_query) = &self.default_query {
			for (key, value) in default_query {
				query.insert(key.clone(), value.clone());
			}
		}
		if !query.is_empty() {
			let search = query
				.iter()
				.map(|(key, value)| format!("{}={}", qs_encode(key), qs_encode(value)))
				.collect::<Vec<_>>()
				.join("&");
			url.set_query(if search.is_empty() { None } else { Some(&search) });
		}
		Ok(url.to_string())
	}

	/// `makeRequest` and `retryRequest`: sends the request, retrying the failures the SDK retries.
	async fn make_request(
		&self,
		method: &str,
		path: &str,
		body: &Value,
		options: &RequestOptions,
	) -> PiResult<HttpResponse> {
		let max_retries = options.max_retries.unwrap_or(self.max_retries);
		let mut retries_remaining = max_retries;
		let caller_signal = options.signal.as_ref();
		loop {
			let (request, timeout) =
				self.build_request(method, path, body, options, max_retries - retries_remaining)?;
			if caller_signal.is_some_and(AbortSignal::aborted) {
				return Err(user_abort_error());
			}
			let response = match self.fetch_with_timeout(request, timeout, caller_signal).await {
				Ok(response) => response,
				Err(error) => {
					if caller_signal.is_some_and(AbortSignal::aborted) {
						return Err(user_abort_error());
					}
					let is_timeout = error.is_abort()
						|| TIMED_OUT.is_match(&format!("{}{}", error_to_string(&error), error_cause_string(&error)));
					if is_retryable_count(retries_remaining) {
						self.retry_sleep(options, retries_remaining, max_retries, None).await?;
						retries_remaining -= 1.0;
						continue;
					}
					if is_timeout {
						return Err(connection_error("Request timed out."));
					}
					return Err(connection_error("Connection error."));
				}
			};

			if response.ok() {
				return Ok(response);
			}

			let should_retry = should_retry(&response);
			if is_retryable_count(retries_remaining) && should_retry {
				let headers = response.headers.clone();
				drop(response);
				self.retry_sleep(options, retries_remaining, max_retries, Some(&headers))
					.await?;
				retries_remaining -= 1.0;
				continue;
			}

			let status = response.status;
			let headers = response.headers.clone();
			let err_text = match response.text().await {
				Ok(text) => text,
				Err(error) => error.message,
			};
			let err_json = json_parse(&err_text).ok();
			let err_message = if err_json.as_ref().is_some_and(is_truthy) {
				None
			} else {
				Some(err_text)
			};
			return Err(make_status_error(status, err_json, err_message, &headers));
		}
	}

	/// `buildRequest`: the request and its timeout.
	fn build_request(
		&self,
		method: &str,
		path: &str,
		body: &Value,
		options: &RequestOptions,
		retry_count: f64,
	) -> PiResult<(HttpRequest, f64)> {
		let url = self.build_url(path)?;
		if let Some(timeout) = options.timeout {
			validate_positive_integer("timeout", timeout)?;
		}
		let timeout = options.timeout.unwrap_or(self.timeout);

		let mut headers = NullableHeaders::default();
		headers.set("Accept", Some("application/json"))?;
		headers.set("User-Agent", Some(&format!("OpenAI/JS {VERSION}")))?;
		headers.set("X-Stainless-Retry-Count", Some(&number_to_string(retry_count)))?;
		if timeout != 0.0 && !timeout.is_nan() {
			headers.set(
				"X-Stainless-Timeout",
				Some(&number_to_string((timeout / 1000.0).trunc())),
			)?;
		}
		for (name, value) in platform_headers() {
			headers.set(name, Some(&value))?;
		}
		headers.set("OpenAI-Organization", self.organization.as_deref())?;
		headers.set("OpenAI-Project", self.project.as_deref())?;
		if let Some(api_key) = &self.api_key {
			headers.set("Authorization", Some(&format!("Bearer {api_key}")))?;
		}
		match &self.default_headers {
			DefaultHeaders::Object(rows) => {
				for (name, value) in rows {
					headers.set(name, value.as_deref())?;
				}
			}
			DefaultHeaders::Built(built) => headers.extend(built)?,
		}
		headers.set("content-type", Some("application/json"))?;
		for (name, value) in options.headers.iter().flatten() {
			headers.set(name, value.as_deref())?;
		}
		headers.validate_auth()?;

		let mut request = HttpRequest::new(method.to_uppercase(), url);
		request.headers = headers.into_request_headers();
		request.body = Some(Bytes::from(json_stringify(body)));
		request.signal = options.signal.clone();
		if self.manual_redirect_with_api_key && request.header("api-key").is_some() {
			request.redirect = pi_host::RedirectMode::Manual;
		}
		Ok((request, timeout))
	}

	/// `fetchWithTimeout`: aborts the request after `timeout` ms or when the caller aborts.
	async fn fetch_with_timeout(
		&self,
		mut request: HttpRequest,
		timeout: f64,
		caller_signal: Option<&AbortSignal>,
	) -> PiResult<HttpResponse> {
		let controller = AbortController::new();
		request.signal = Some(match caller_signal {
			Some(caller_signal) => AbortSignal::any(&[controller.signal(), caller_signal.clone()]),
			None => controller.signal(),
		});
		let fetch = pi_host::fetch(request, self.fetch.as_ref());
		let timer = pi_host::current().sleep(timeout);
		let caller_aborted = async {
			match caller_signal {
				Some(signal) => signal.aborted_future().await,
				None => futures::future::pending().await,
			}
		};
		futures::pin_mut!(fetch, timer, caller_aborted);
		match select(fetch, select(timer, caller_aborted)).await {
			Either::Left((response, _)) => response,
			Either::Right((Either::Left(_), _)) => {
				controller.abort();
				Err(PiError::abort())
			}
			Either::Right((Either::Right((reason, _)), _)) => Err(reason),
		}
	}

	/// `retryRequest`'s wait before the next attempt.
	async fn retry_sleep(
		&self,
		options: &RequestOptions,
		retries_remaining: f64,
		max_retries: f64,
		response_headers: Option<&IndexMap<String, String>>,
	) -> PiResult<()> {
		let header = |name: &str| {
			response_headers
				.and_then(|headers| headers.get(name))
				.filter(|value| !value.is_empty())
		};
		let mut timeout_millis: Option<f64> = None;
		if let Some(retry_after_ms) = header("retry-after-ms") {
			let timeout_ms = parse_float(retry_after_ms);
			if !timeout_ms.is_nan() {
				timeout_millis = Some(timeout_ms);
			}
		}
		if let Some(retry_after) = header("retry-after")
			&& timeout_millis.is_none()
		{
			let timeout_seconds = parse_float(retry_after);
			timeout_millis = Some(if timeout_seconds.is_nan() {
				date_parse(retry_after) - pi_host::now_ms()
			} else {
				timeout_seconds * 1000.0
			});
		}
		let timeout_millis = match timeout_millis {
			Some(timeout) if (0.0..=60.0 * 1000.0).contains(&timeout) => timeout,
			_ => calculate_default_retry_timeout_millis(retries_remaining, max_retries),
		};
		pi_host::sleep(timeout_millis, options.signal.as_ref())
			.await
			.map_err(|_| user_abort_error())
	}
}

/// `retriesRemaining` as a JS condition.
fn is_retryable_count(retries_remaining: f64) -> bool {
	retries_remaining != 0.0 && !retries_remaining.is_nan()
}

static TIMED_OUT: LazyLock<Regex> = LazyLock::new(|| Regex::new("(?i)timed? ?out").expect("valid regex"));

/// `String(error)`: `Error.prototype.toString`.
fn error_to_string(error: &PiError) -> String {
	match (error.name.is_empty(), error.message.is_empty()) {
		(true, _) => error.message.clone(),
		(false, true) => error.name.clone(),
		(false, false) => format!("{}: {}", error.name, error.message),
	}
}

/// `'cause' in error ? String(error.cause) : ''`.
fn error_cause_string(error: &PiError) -> String {
	match error.data.as_ref().and_then(|data| data.get("cause")) {
		None => String::new(),
		Some(Value::String(cause)) => cause.clone(),
		Some(Value::Object(cause)) => match (cause.get("name"), cause.get("message")) {
			(Some(Value::String(name)), Some(Value::String(message))) => {
				error_to_string(&PiError::named(name.clone(), message.clone()))
			}
			_ => "[object Object]".into(),
		},
		Some(Value::Null) => "null".into(),
		Some(cause) => json_stringify(cause),
	}
}

fn should_retry(response: &HttpResponse) -> bool {
	// Note this is not a standard header.
	match response.header("x-should-retry") {
		Some("true") => return true,
		Some("false") => return false,
		_ => {}
	}
	let status = response.status;
	status == 408 || status == 409 || status == 429 || status >= 500
}

fn calculate_default_retry_timeout_millis(retries_remaining: f64, max_retries: f64) -> f64 {
	let initial_retry_delay = 0.5;
	let max_retry_delay = 8.0;
	let num_retries = max_retries - retries_remaining;
	let sleep_seconds = pi_host::js::math_min(initial_retry_delay * 2f64.powf(num_retries), max_retry_delay);
	let jitter = 1.0 - rand::random::<f64>() * 0.25;
	sleep_seconds * jitter * 1000.0
}

fn validate_positive_integer(name: &str, value: f64) -> PiResult<()> {
	if !value.is_finite() || value.trunc() != value {
		return Err(PiError::new(format!("{name} must be an integer")));
	}
	if value < 0.0 {
		return Err(PiError::new(format!("{name} must be a positive integer")));
	}
	Ok(())
}

/// JS truthiness of a JSON value.
pub(crate) fn is_truthy(value: &Value) -> bool {
	match value {
		Value::Null => false,
		Value::Bool(value) => *value,
		Value::Number(number) => number.as_f64().is_some_and(|number| number != 0.0 && !number.is_nan()),
		Value::String(value) => !value.is_empty(),
		Value::Array(_) | Value::Object(_) => true,
	}
}

/// `APIUserAbortError`.
fn user_abort_error() -> PiError {
	api_error_without_response("Request was aborted.")
}

/// `APIConnectionError` and `APIConnectionTimeoutError`.
fn connection_error(message: &str) -> PiError {
	api_error_without_response(message)
}

/// An `APIError` with `status` and `headers` undefined.
fn api_error_without_response(message: &str) -> PiError {
	PiError::new(message).with_data(serde_json::json!({ "headers": null }))
}

/// `APIError.makeMessage(status, error, message)`.
fn make_message(status: Option<u16>, error: Option<&Value>, message: Option<&str>) -> String {
	let msg = match error {
		Some(error) => match error
			.get("message")
			.filter(|message| error.is_object() && is_truthy(message))
		{
			Some(Value::String(message)) => Some(message.clone()),
			Some(message) => Some(json_stringify(message)),
			None if is_truthy(error) => Some(json_stringify(error)),
			None => message.map(str::to_string),
		},
		None => message.map(str::to_string),
	};
	let msg = msg.filter(|msg| !msg.is_empty());
	match (status, msg) {
		(Some(status), Some(msg)) => format!("{status} {msg}"),
		(Some(status), None) => format!("{status} status code (no body)"),
		(None, Some(msg)) => msg,
		(None, None) => "(no status code or body)".into(),
	}
}

/// `new APIError(status, error, message, headers)` as a `PiError`.
fn api_error(
	status: Option<u16>,
	error: Option<Value>,
	message: Option<&str>,
	headers: &IndexMap<String, String>,
) -> PiError {
	let text = make_message(status, error.as_ref(), message);
	let record = headers_to_record(headers);
	let mut data = Map::new();
	data.insert(
		"headers".into(),
		Value::Object(
			record
				.iter()
				.map(|(name, value)| (name.clone(), Value::String(value.clone())))
				.collect(),
		),
	);
	if let Some(error) = &error {
		data.insert("error".into(), error.clone());
		for field in ["code", "param", "type"] {
			if let Some(value) = error.as_object().and_then(|error| error.get(field)) {
				data.insert(field.into(), value.clone());
			}
		}
	}
	data.insert(
		"requestID".into(),
		record
			.get("x-request-id")
			.map_or(Value::Null, |id| Value::String(id.clone())),
	);
	let error = PiError::new(text).with_data(Value::Object(data));
	match status {
		Some(status) => error.with_status(status),
		None => error,
	}
}

/// `makeStatusError` and `APIError.generate`.
fn make_status_error(
	status: u16,
	error: Option<Value>,
	message: Option<String>,
	headers: &IndexMap<String, String>,
) -> PiError {
	let error = match error {
		Some(Value::Object(mut body)) => match body.get("error") {
			Some(inner) if !inner.is_null() => body.shift_remove("error"),
			_ => Some(Value::Object(body)),
		},
		Some(Value::Array(items)) => Some(Value::Array(items)),
		_ => None,
	};
	api_error(Some(status), error, message.as_deref(), headers)
}

/// `defaultParseResponse` for a response that is not a stream.
async fn default_parse_response(response: HttpResponse) -> PiResult<Value> {
	if response.status == 204 {
		return Ok(Value::Null);
	}
	let media_type = response
		.header("content-type")
		.and_then(|content_type| content_type.split(';').next())
		.map(|media_type| crate::utils::text::js_trim(media_type).to_lowercase());
	let is_json = media_type
		.as_deref()
		.is_some_and(|media_type| media_type.contains("application/json") || media_type.ends_with("+json"));
	if is_json {
		if response.header("content-length") == Some("0") {
			return Ok(Value::Null);
		}
		let text = response.text().await?;
		if text.is_empty() {
			return Ok(Value::Null);
		}
		return json_parse(&text);
	}
	Ok(Value::String(response.text().await?))
}

/// `Stream.fromSSEResponse(response, controller)`: the parsed JSON of each event until `[DONE]`.
pub fn stream_from_sse_response(
	response: HttpResponse,
	signal: Option<AbortSignal>,
) -> LocalBoxStream<'static, PiResult<Value>> {
	let headers = response.headers.clone();
	let body_signal = signal.clone();
	let body = response.body;
	let body = futures::stream::unfold((body, body_signal), |(mut body, signal)| async move {
		let next = match &signal {
			Some(signal) => {
				if signal.aborted() {
					return None;
				}
				let aborted = signal.aborted_future();
				futures::pin_mut!(aborted);
				match select(body.next(), aborted).await {
					Either::Left((next, _)) => next,
					Either::Right(_) => return None,
				}
			}
			None => body.next().await,
		};
		next.map(|item| (item, (body, signal)))
	});
	let mut events = pi_host::sse::decode(body);
	pi_host::sse::async_stream(move |yield_| async move {
		while let Some(event) = events.next().await {
			if signal.as_ref().is_some_and(AbortSignal::aborted) {
				return;
			}
			let event = match event {
				Ok(event) => event,
				// Abort errors from the transport end the stream without an error.
				Err(error) if error.is_abort() => return,
				Err(error) => {
					yield_(Err(error)).await;
					return;
				}
			};
			if event.data == "[DONE]" {
				return;
			}
			let data = match json_parse(&event.data) {
				Ok(data) => data,
				Err(_) => {
					yield_(Err(PiError::named(
						"SyntaxError",
						"Error reading response: malformed server-sent event JSON.",
					)))
					.await;
					return;
				}
			};
			let event_name = event.event.clone();
			if event_name.as_deref().is_some_and(|name| name.starts_with("thread.")) {
				yield_(Ok(serde_json::json!({ "event": event_name, "data": data }))).await;
				continue;
			}
			if event_name.as_deref() == Some("error") {
				let error = match data.get("error") {
					Some(error) if !error.is_null() => error.clone(),
					_ => data,
				};
				yield_(Err(api_error(None, Some(error), None, &headers))).await;
				return;
			}
			if let Some(error) = data.get("error").filter(|error| is_truthy(&data) && is_truthy(error)) {
				yield_(Err(api_error(None, Some(error.clone()), None, &headers))).await;
				return;
			}
			yield_(Ok(data)).await;
		}
	})
}

/// `options.defaultHeaders`: the object the caller passed, or the headers the constructor built from it.
#[derive(Clone, Debug)]
enum DefaultHeaders {
	Object(ProviderHeaders),
	Built(NullableHeaders),
}

/// `NullableHeaders`: header values by lowercase name, and the names explicitly set to `null`.
#[derive(Clone, Debug, Default)]
struct NullableHeaders {
	values: IndexMap<String, String>,
	nulls: IndexSet<String>,
}

static HTTP_TOKEN_HEADER_NAME: LazyLock<Regex> =
	LazyLock::new(|| Regex::new(r"^[!#$%&'*+\-.^_`|~0-9A-Za-z]+$").expect("valid regex"));

impl NullableHeaders {
	/// One `[name, value]` row of `buildHeaders` from an object source: later rows replace earlier ones.
	fn set(&mut self, name: &str, value: Option<&str>) -> PiResult<()> {
		if !HTTP_TOKEN_HEADER_NAME.is_match(name) {
			return Err(PiError::type_error(format!(
				"Header name must be a valid HTTP token [\"{name}\"]"
			)));
		}
		let lower_name = name.to_ascii_lowercase();
		self.values.shift_remove(&lower_name);
		match value {
			None => {
				self.nulls.insert(lower_name);
			}
			Some(value) => {
				let value = value.trim_matches([' ', '\t', '\n', '\r']);
				if value.contains(['\0', '\r', '\n']) {
					return Err(PiError::type_error(format!(
						"Headers.append: \"{value}\" is an invalid header value."
					)));
				}
				self.values.insert(lower_name.clone(), value.to_string());
				self.nulls.shift_remove(&lower_name);
			}
		}
		Ok(())
	}

	/// A `NullableHeaders` source of `buildHeaders`: its values in `Headers` order, then its nulls.
	fn extend(&mut self, other: &NullableHeaders) -> PiResult<()> {
		let mut names: Vec<&String> = other.values.keys().collect();
		names.sort();
		for name in names {
			self.set(name, Some(&other.values[name]))?;
		}
		for name in &other.nulls {
			self.set(name, None)?;
		}
		Ok(())
	}

	/// `validateHeaders` for bearer authentication.
	fn validate_auth(&self) -> PiResult<()> {
		let has_value = |name: &str| self.values.get(name).is_some_and(|value| !value.is_empty());
		if has_value("authorization") || has_value("api-key") {
			return Ok(());
		}
		if self.nulls.contains("authorization") || self.nulls.contains("api-key") {
			return Ok(());
		}
		Err(PiError::new(
			"Could not resolve authentication method. Expected either apiKey or adminAPIKey to be set. Or for one of the \"Authorization\" or \"api-key\" headers to be explicitly omitted",
		))
	}

	/// The headers a `Headers` object hands to `fetch`: lowercase names in sorted order.
	fn into_request_headers(self) -> IndexMap<String, String> {
		let mut values = self.values;
		values.sort_keys();
		values
	}
}

/// `getPlatformHeaders()` for the Rust build target.
fn platform_headers() -> [(&'static str, String); 6] {
	[
		("X-Stainless-Lang", "js".into()),
		("X-Stainless-Package-Version", VERSION.into()),
		("X-Stainless-OS", normalize_platform(std::env::consts::OS)),
		("X-Stainless-Arch", normalize_arch(std::env::consts::ARCH)),
		("X-Stainless-Runtime", "unknown".into()),
		("X-Stainless-Runtime-Version", "unknown".into()),
	]
}

fn normalize_arch(arch: &str) -> String {
	match arch {
		"x32" => "x32".into(),
		"x86_64" | "x64" => "x64".into(),
		"arm" => "arm".into(),
		"aarch64" | "arm64" => "arm64".into(),
		"" => "unknown".into(),
		arch => format!("other:{arch}"),
	}
}

fn normalize_platform(platform: &str) -> String {
	let platform = platform.to_lowercase();
	if platform.contains("ios") {
		return "iOS".into();
	}
	match platform.as_str() {
		"android" => "Android".into(),
		"darwin" | "macos" => "MacOS".into(),
		"win32" | "windows" => "Windows".into(),
		"freebsd" => "FreeBSD".into(),
		"openbsd" => "OpenBSD".into(),
		"linux" => "Linux".into(),
		"" => "Unknown".into(),
		platform => format!("Other:{platform}"),
	}
}

/// The `qs` encoder (RFC 3986): every byte of the UTF-8 form except `-._~` and ASCII alphanumerics is
/// percent-encoded with uppercase hex digits.
fn qs_encode(text: &str) -> String {
	let mut out = String::with_capacity(text.len());
	for byte in text.bytes() {
		if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~') {
			out.push(byte as char);
		} else {
			out.push_str(&format!("%{byte:02X}"));
		}
	}
	out
}

/// Keeps `Rc` in the public surface for callers that share a client.
pub type SharedOpenAI = Rc<OpenAI>;
