//! Rust-only replacement for the parts of `@google/genai` (v2.21.0) that pi-ai uses: the `GoogleGenAI` constructor
//! (backend selection, API key and Vertex project/location precedence, env fallbacks), the `ApiClient` (base URL and
//! API version, default and auth headers, `X-Server-Timeout`, `extraBody`), `models.generateContentStream` with its
//! request converters (`generateContentParametersToMldev` / `ToVertex` and the part, tool, tool-config, schema and
//! speech converters they call, including the "only supported in ... mode" errors) and response converters,
//! `throwErrorIfNotOK` / `ApiError`, and the SDK's own SSE splitter (`processStreamResponse`, which is not a standard
//! SSE parser, so `pi_host::sse` is not used). Vertex ADC lives in `google_auth.rs`.
//! PORT-STATUS: reviewed
//!
//! Request parameters are the JSON `GenerateContentParameters` value the adapters build and `onPayload` may replace;
//! `config.abortSignal` is not JSON, so the signal is a separate argument. Header records are JS objects
//! (case-sensitive keys, insertion order) until `getHeadersInternal` appends them to a `Headers`, which lowercases
//! names and joins repeated names with `", "`. Differences from the SDK:
//! - The `gl-node/<process.version>` label in `User-Agent` and `x-goog-api-client` is `gl-rust/<rust-version>`, the
//!   runtime `utils/identity.rs` reports for the same reason.
//! - `HttpOptions.retryOptions` (p-retry) is not ported: pi-ai never sets it, so requests are sent once, as with the
//!   SDK default. Callable and MCP tools cannot be JSON, so automatic function calling is always disabled, as the SDK
//!   decides for plain tool declarations; the `inputSchema` MCP detection and its `mcp_used` header are kept.
//! - `tTool` rewrites `functionDeclarations[].parameters` in place in the SDK; here it rewrites the request copy.
//! - Request headers are sent in insertion order; a `Headers` object iterates them sorted. Header values are trimmed
//!   like `Headers.append`, but invalid values are not rejected.
//! - A truthy `config.tools` that is not an array fails in `tTools` ("tools is required and must be an array of
//!   Tools") instead of with the `TypeError` the SDK's `shouldDisableAfc` / `isCallableTool` throw for it.
//! - A host response always has a body stream, so an ok response without a body yields no chunks instead of failing
//!   with `Response body is empty`.

use std::rc::Rc;

use futures::StreamExt;
use indexmap::IndexMap;
use pi_host::js::{js_key_order, json_stringify, number_to_string, string_to_number};
use pi_host::sse::async_stream;
use pi_host::{AbortController, AbortSignal, HttpRequest, HttpResponse, LocalBoxStream, PiError, PiResult};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

use super::google_auth::{GoogleAuth, GoogleAuthOptions};
use crate::utils::json_parse::json_parse;
use crate::utils::text::js_trim;

/// `SDK_VERSION`.
pub const SDK_VERSION: &str = "2.21.0";
const CONTENT_TYPE_HEADER: &str = "Content-Type";
const SERVER_TIMEOUT_HEADER: &str = "X-Server-Timeout";
const USER_AGENT_HEADER: &str = "User-Agent";
const GOOGLE_API_CLIENT_HEADER: &str = "x-goog-api-client";
const GOOGLE_API_KEY_HEADER: &str = "x-goog-api-key";
const VERTEX_AI_API_DEFAULT_VERSION: &str = "v1beta1";
const GOOGLE_AI_API_DEFAULT_VERSION: &str = "v1beta";
const MULTI_REGIONAL_LOCATIONS: [&str; 2] = ["us", "eu"];
const MCP_LABEL: &str = "mcp_used/unknown";
const TYPE_VALUES: [&str; 8] = [
	"TYPE_UNSPECIFIED",
	"STRING",
	"NUMBER",
	"INTEGER",
	"BOOLEAN",
	"ARRAY",
	"OBJECT",
	"NULL",
];

/// `FunctionCallingConfigMode`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum FunctionCallingConfigMode {
	ModeUnspecified,
	Auto,
	Any,
	None,
	Validated,
}

impl FunctionCallingConfigMode {
	/// The wire value.
	pub fn as_str(self) -> &'static str {
		match self {
			FunctionCallingConfigMode::ModeUnspecified => "MODE_UNSPECIFIED",
			FunctionCallingConfigMode::Auto => "AUTO",
			FunctionCallingConfigMode::Any => "ANY",
			FunctionCallingConfigMode::None => "NONE",
			FunctionCallingConfigMode::Validated => "VALIDATED",
		}
	}
}

/// `ThinkingLevel`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ThinkingLevel {
	ThinkingLevelUnspecified,
	Minimal,
	Low,
	Medium,
	High,
}

impl ThinkingLevel {
	/// The wire value.
	pub fn as_str(self) -> &'static str {
		match self {
			ThinkingLevel::ThinkingLevelUnspecified => "THINKING_LEVEL_UNSPECIFIED",
			ThinkingLevel::Minimal => "MINIMAL",
			ThinkingLevel::Low => "LOW",
			ThinkingLevel::Medium => "MEDIUM",
			ThinkingLevel::High => "HIGH",
		}
	}
}

/// `ResourceScope`: how a custom `baseUrl` relates to the Vertex project path.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ResourceScope {
	/// The base URL already names the collection, so no `projects/<p>/locations/<l>` prefix is added.
	#[serde(rename = "COLLECTION")]
	Collection,
}

/// `HttpOptions` (the fields pi-ai and the request converters use).
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HttpOptions {
	#[serde(skip_serializing_if = "Option::is_none")]
	pub base_url: Option<String>,
	#[serde(skip_serializing_if = "Option::is_none")]
	pub base_url_resource_scope: Option<ResourceScope>,
	#[serde(skip_serializing_if = "Option::is_none")]
	pub api_version: Option<String>,
	#[serde(skip_serializing_if = "Option::is_none")]
	pub headers: Option<IndexMap<String, String>>,
	/// Per-attempt timeout in milliseconds.
	#[serde(skip_serializing_if = "Option::is_none")]
	pub timeout: Option<f64>,
	#[serde(skip_serializing_if = "Option::is_none")]
	pub extra_body: Option<Map<String, Value>>,
}

/// `GoogleGenAIOptions`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct GoogleGenAIOptions {
	pub vertexai: Option<bool>,
	pub api_key: Option<String>,
	pub project: Option<String>,
	pub location: Option<String>,
	pub api_version: Option<String>,
	pub google_auth_options: Option<GoogleAuthOptions>,
	pub http_options: Option<HttpOptions>,
}

/// `NodeAuth`: an API key or Google auth (ADC).
enum Auth {
	ApiKey(String),
	Google(GoogleAuth),
}

struct ApiClient {
	vertexai: bool,
	project: Option<String>,
	location: Option<String>,
	http_options: Map<String, Value>,
	auth: Auth,
}

/// `GoogleGenAI` with its `models.generateContentStream`.
#[derive(Clone)]
pub struct GoogleGenAI {
	api_client: Rc<ApiClient>,
}

/// One `GenerateContentResponse`, as JSON, with `sdkHttpResponse.headers` set.
pub type GenerateContentStream = LocalBoxStream<'static, PiResult<Value>>;

fn truthy(value: Option<&str>) -> bool {
	value.is_some_and(|value| !value.is_empty())
}

/// `getEnv`: `process.env[name]?.trim()`.
fn get_env(name: &str) -> Option<String> {
	pi_host::env(name).map(|value| js_trim(&value).to_string())
}

fn user_agent_extra() -> String {
	format!("gl-rust/{}", env!("CARGO_PKG_RUST_VERSION"))
}

impl GoogleGenAI {
	/// `new GoogleGenAI(options)`.
	pub fn new(options: GoogleGenAIOptions) -> PiResult<Self> {
		let vertexai = resolve_cloud_flag(options.vertexai);
		if (truthy(options.project.as_deref()) || truthy(options.location.as_deref())) && !vertexai {
			return Err(PiError::new(
				"Project and location are not supported for Gemini API backend.",
			));
		}
		let env_api_key = get_api_key_from_env();
		let env_project = get_env("GOOGLE_CLOUD_PROJECT");
		let env_location = get_env("GOOGLE_CLOUD_LOCATION");
		let mut api_key = options.api_key.clone().or(env_api_key.clone());
		let mut project = options.project.clone().or(env_project.clone());
		let mut location = options.location.clone().or(env_location.clone());
		if vertexai {
			let explicit_project_or_location =
				truthy(options.project.as_deref()) || truthy(options.location.as_deref());
			let env_project_or_location = truthy(env_project.as_deref()) || truthy(env_location.as_deref());
			if !explicit_project_or_location && env_project_or_location && truthy(options.api_key.as_deref()) {
				project = None;
				location = None;
			} else if (explicit_project_or_location
				&& !truthy(options.api_key.as_deref())
				&& truthy(env_api_key.as_deref()))
				|| (!explicit_project_or_location
					&& !truthy(options.api_key.as_deref())
					&& env_project_or_location
					&& truthy(env_api_key.as_deref()))
			{
				api_key = None;
			}
			if !truthy(location.as_deref()) && !truthy(api_key.as_deref()) {
				location = Some("global".into());
			}
		}
		let mut http_options = options.http_options.clone();
		let base_url = get_base_url(
			http_options.as_ref(),
			vertexai,
			get_env("GOOGLE_VERTEX_BASE_URL"),
			get_env("GOOGLE_GEMINI_BASE_URL"),
		);
		if let Some(base_url) = base_url.filter(|base_url| !base_url.is_empty()) {
			http_options.get_or_insert_with(HttpOptions::default).base_url = Some(base_url);
		}
		let auth = match &api_key {
			Some(api_key) => Auth::ApiKey(api_key.clone()),
			None => Auth::Google(GoogleAuth::new(build_google_auth_options(options.google_auth_options)?)),
		};
		let api_client = ApiClient::new(ApiClientOptions {
			auth,
			project,
			location,
			api_version: options.api_version,
			api_key,
			vertexai,
			http_options,
		})?;
		Ok(Self {
			api_client: Rc::new(api_client),
		})
	}

	/// `models.generateContentStream(params)`. `signal` is `params.config.abortSignal`.
	pub async fn generate_content_stream(
		&self,
		mut params: Value,
		signal: Option<AbortSignal>,
	) -> PiResult<GenerateContentStream> {
		maybe_move_to_response_json_schema(&mut params);
		let params = self.process_params_maybe_add_mcp_usage(params);
		self.generate_content_stream_internal(params, signal).await
	}

	fn process_params_maybe_add_mcp_usage(&self, params: Value) -> Value {
		let Some(tools) = params
			.get("config")
			.and_then(|config| config.get("tools"))
			.filter(|tools| js_truthy(tools))
		else {
			return params;
		};
		let has_mcp_tool_usage = tools
			.as_array()
			.is_some_and(|tools| tools.iter().any(|tool| tool.get("inputSchema").is_some()));
		let mut config = params
			.get("config")
			.and_then(Value::as_object)
			.cloned()
			.unwrap_or_default();
		config.insert("tools".into(), tools.clone());
		if has_mcp_tool_usage {
			let headers = config
				.get("httpOptions")
				.and_then(|http_options| http_options.get("headers"))
				.and_then(Value::as_object)
				.cloned()
				.unwrap_or_default();
			let mut headers = if headers.is_empty() {
				self.api_client.default_headers()
			} else {
				headers
			};
			let existing = headers
				.get(GOOGLE_API_CLIENT_HEADER)
				.map(header_value_string)
				.unwrap_or_default();
			headers.insert(
				GOOGLE_API_CLIENT_HEADER.into(),
				Value::String(format!("{existing} {MCP_LABEL}").trim_start().to_string()),
			);
			let mut http_options = config
				.get("httpOptions")
				.and_then(Value::as_object)
				.cloned()
				.unwrap_or_default();
			http_options.insert("headers".into(), Value::Object(headers));
			config.insert("httpOptions".into(), Value::Object(http_options));
		}
		let mut next = Map::new();
		for key in ["model", "contents"] {
			if let Some(value) = params.get(key) {
				next.insert(key.into(), value.clone());
			}
		}
		next.insert("config".into(), Value::Object(config));
		Value::Object(next)
	}

	async fn generate_content_stream_internal(
		&self,
		params: Value,
		signal: Option<AbortSignal>,
	) -> PiResult<GenerateContentStream> {
		let vertexai = self.api_client.vertexai;
		let mut body = if vertexai {
			generate_content_parameters_to_vertex(&self.api_client, &params)?
		} else {
			generate_content_parameters_to_mldev(&self.api_client, &params)?
		};
		let url_values = body.shift_remove("_url");
		body.shift_remove("_query");
		let path = format_model_path(url_values.as_ref())?;
		let http_options = params
			.get("config")
			.and_then(|config| config.get("httpOptions"))
			.cloned();
		let chunks = self
			.api_client
			.request_stream(
				&path,
				json_stringify(&Value::Object(body)),
				http_options.as_ref(),
				signal,
			)
			.await?;
		Ok(chunks
			.map(move |chunk| {
				let (json, headers) = chunk?;
				let mut response = if vertexai {
					generate_content_response_from_vertex(&json)?
				} else {
					generate_content_response_from_mldev(&json)?
				};
				// `response.headers.entries()` iterates lowercase names in sorted order.
				let mut headers: Vec<(String, String)> = headers
					.into_iter()
					.map(|(name, value)| (name.to_ascii_lowercase(), value))
					.collect();
				headers.sort_by(|(a, _), (b, _)| a.cmp(b));
				let headers: Map<String, Value> = headers
					.into_iter()
					.map(|(name, value)| (name, Value::String(value)))
					.collect();
				response.insert("sdkHttpResponse".into(), json!({ "headers": headers }));
				Ok(Value::Object(response))
			})
			.boxed_local())
	}
}

/// `formatMap('{model}:streamGenerateContent?alt=sse', body._url)`.
fn format_model_path(url_values: Option<&Value>) -> PiResult<String> {
	let Some(url_values) = url_values.and_then(Value::as_object) else {
		return Err(PiError::type_error("Cannot convert undefined or null to object"));
	};
	let Some(model) = url_values.get("model") else {
		return Err(PiError::new("Key 'model' not found in valueMap."));
	};
	Ok(format!("{}:streamGenerateContent?alt=sse", js_string(model)))
}

/// `resolveCloudFlag(options)` for a client whose `enterprise` option is never set.
fn resolve_cloud_flag(vertexai: Option<bool>) -> bool {
	if let Some(vertexai) = vertexai {
		return vertexai;
	}
	let env_enterprise = get_env("GOOGLE_GENAI_USE_ENTERPRISE");
	let env_vertexai = get_env("GOOGLE_GENAI_USE_VERTEXAI");
	let to_bool = |value: &str| value.to_lowercase() == "true";
	if let Some(value) = env_enterprise {
		return to_bool(&value);
	}
	if let Some(value) = env_vertexai {
		return to_bool(&value);
	}
	false
}

/// `getApiKeyFromEnv()`.
fn get_api_key_from_env() -> Option<String> {
	let google = get_env("GOOGLE_API_KEY").filter(|value| !value.is_empty());
	let gemini = get_env("GEMINI_API_KEY").filter(|value| !value.is_empty());
	google.or(gemini)
}

/// `getBaseUrl(httpOptions, vertexai, vertexBaseUrlFromEnv, geminiBaseUrlFromEnv)` (no `setDefaultBaseUrls` call).
fn get_base_url(
	http_options: Option<&HttpOptions>,
	vertexai: bool,
	vertex_base_url_from_env: Option<String>,
	gemini_base_url_from_env: Option<String>,
) -> Option<String> {
	match http_options.and_then(|options| options.base_url.clone()) {
		Some(base_url) if !base_url.is_empty() => Some(base_url),
		_ if vertexai => vertex_base_url_from_env,
		_ => gemini_base_url_from_env,
	}
}

/// `buildGoogleAuthOptions` of `NodeAuth`: the Vertex scope is required.
fn build_google_auth_options(options: Option<GoogleAuthOptions>) -> PiResult<GoogleAuthOptions> {
	let mut options = options.unwrap_or_default();
	match &options.scopes {
		None => options.scopes = Some(vec![super::google_auth::REQUIRED_VERTEX_AI_SCOPE.to_string()]),
		Some(scopes)
			if !scopes
				.iter()
				.any(|scope| scope == super::google_auth::REQUIRED_VERTEX_AI_SCOPE) =>
		{
			return Err(PiError::new(format!(
				"Invalid auth scopes. Scopes must include: {}",
				super::google_auth::REQUIRED_VERTEX_AI_SCOPE
			)));
		}
		Some(_) => {}
	}
	Ok(options)
}

struct ApiClientOptions {
	auth: Auth,
	project: Option<String>,
	location: Option<String>,
	api_version: Option<String>,
	api_key: Option<String>,
	vertexai: bool,
	http_options: Option<HttpOptions>,
}

impl ApiClient {
	fn new(opts: ApiClientOptions) -> PiResult<Self> {
		let custom_base_url = opts.http_options.as_ref().and_then(|options| options.base_url.clone());
		let mut project = opts.project.clone();
		let mut location = opts.location.clone();
		let mut init = Map::new();
		if opts.vertexai {
			if !truthy(location.as_deref()) && !truthy(opts.api_key.as_deref()) && !truthy(custom_base_url.as_deref()) {
				location = Some("global".into());
			}
			let has_sufficient_auth =
				(truthy(project.as_deref()) && truthy(location.as_deref())) || truthy(opts.api_key.as_deref());
			if !has_sufficient_auth && !truthy(custom_base_url.as_deref()) {
				return Err(PiError::new(
					"Authentication is not set up. Please provide either a project and location, or an API key, or a custom base URL.",
				));
			}
			let has_constructor_auth = (truthy(opts.project.as_deref()) && truthy(opts.location.as_deref()))
				|| truthy(opts.api_key.as_deref());
			let regional = truthy(project.as_deref()) && truthy(location.as_deref());
			if truthy(custom_base_url.as_deref()) && !has_constructor_auth {
				init.insert("baseUrl".into(), json!(custom_base_url));
				project = None;
				location = None;
			} else if (truthy(opts.api_key.as_deref()) && !truthy(project.as_deref()))
				|| location.as_deref() == Some("global")
			{
				init.insert("baseUrl".into(), json!("https://aiplatform.googleapis.com/"));
			} else if let (true, Some(location)) = (regional, location.as_deref()) {
				let base_url = if MULTI_REGIONAL_LOCATIONS.contains(&location) {
					format!("https://aiplatform.{location}.rep.googleapis.com/")
				} else {
					format!("https://{location}-aiplatform.googleapis.com/")
				};
				init.insert("baseUrl".into(), json!(base_url));
			}
			let api_version = opts
				.api_version
				.clone()
				.unwrap_or_else(|| VERTEX_AI_API_DEFAULT_VERSION.into());
			init.insert("apiVersion".into(), json!(api_version));
		} else {
			let api_version = opts
				.api_version
				.clone()
				.unwrap_or_else(|| GOOGLE_AI_API_DEFAULT_VERSION.into());
			init.insert("apiVersion".into(), json!(api_version));
			init.insert("baseUrl".into(), json!("https://generativelanguage.googleapis.com/"));
		}
		init.insert("headers".into(), Value::Object(default_headers()));
		let http_options = match &opts.http_options {
			Some(http_options) => {
				let request = serde_json::to_value(http_options).expect("HttpOptions serializes to JSON");
				patch_http_options(&init, &request)
			}
			None => init,
		};
		Ok(Self {
			vertexai: opts.vertexai,
			project,
			location,
			http_options,
			auth: opts.auth,
		})
	}

	fn default_headers(&self) -> Map<String, Value> {
		default_headers()
	}

	async fn request_stream(
		self: &Rc<Self>,
		path: &str,
		body: String,
		request_http_options: Option<&Value>,
		signal: Option<AbortSignal>,
	) -> PiResult<LocalBoxStream<'static, PiResult<(Value, IndexMap<String, String>)>>> {
		let http_options = match request_http_options {
			Some(request) => patch_http_options(&self.http_options, request),
			None => self.http_options.clone(),
		};
		let prepend_project_location = self.should_prepend_vertex_project_path(path, &http_options);
		let mut url = self.construct_url(path, &http_options, prepend_project_location)?;
		if url
			.query_pairs()
			.find(|(key, _)| key == "alt")
			.is_none_or(|(_, value)| value != "sse")
		{
			set_search_param(&mut url, "alt", "sse");
		}
		let url = url.to_string();
		let body = include_extra_body(body, http_options.get("extraBody"));
		let headers = self.get_headers_internal(&http_options, &url).await?;
		let timeout = http_options.get("timeout").and_then(Value::as_f64);
		let attempt_signal = create_attempt_signal(timeout, signal);
		let response = api_call(&url, body, headers, attempt_signal.clone()).await?;
		let response = throw_error_if_not_ok(response).await?;
		Ok(process_stream_response(response, attempt_signal))
	}

	fn should_prepend_vertex_project_path(&self, path: &str, http_options: &Map<String, Value>) -> bool {
		if http_options.get("baseUrl").is_some_and(js_truthy)
			&& http_options.get("baseUrlResourceScope").and_then(Value::as_str) == Some("COLLECTION")
		{
			return false;
		}
		if !self.vertexai {
			return false;
		}
		if !truthy(self.project.as_deref()) || !truthy(self.location.as_deref()) {
			return false;
		}
		!path.starts_with("projects/")
	}

	fn construct_url(
		&self,
		path: &str,
		http_options: &Map<String, Value>,
		prepend_project_location: bool,
	) -> PiResult<url::Url> {
		let (Some(base_url), Some(api_version)) = (http_options.get("baseUrl"), http_options.get("apiVersion")) else {
			return Err(PiError::new("HTTP options are not correctly set."));
		};
		let base_url = js_string(base_url);
		let base_url = base_url.strip_suffix('/').unwrap_or(&base_url);
		let mut elements = vec![base_url.to_string()];
		let api_version = js_string(api_version);
		if !api_version.is_empty() {
			elements.push(api_version);
		}
		if prepend_project_location {
			elements.push(format!(
				"projects/{}/locations/{}",
				self.project.as_deref().unwrap_or("undefined"),
				self.location.as_deref().unwrap_or("undefined")
			));
		}
		if !path.is_empty() {
			elements.push(path.to_string());
		}
		url::Url::parse(&elements.join("/")).map_err(|_| PiError::type_error("Invalid URL"))
	}

	async fn get_headers_internal(&self, http_options: &Map<String, Value>, url: &str) -> PiResult<Headers> {
		let mut headers = Headers::default();
		if let Some(Value::Object(record)) = http_options.get("headers") {
			for (name, value) in record {
				headers.append(name, &header_value_string(value));
			}
		}
		if let Some(timeout) = http_options
			.get("timeout")
			.and_then(Value::as_f64)
			.filter(|timeout| *timeout > 0.0)
		{
			headers.append(SERVER_TIMEOUT_HEADER, &number_to_string((timeout / 1000.0).ceil()));
		}
		match &self.auth {
			Auth::ApiKey(api_key) => {
				if api_key.starts_with("auth_tokens/") {
					return Err(PiError::new("Ephemeral tokens are only supported by the live API."));
				}
				if headers.get(GOOGLE_API_KEY_HEADER).is_none() {
					headers.append(GOOGLE_API_KEY_HEADER, api_key);
				}
			}
			Auth::Google(auth) => {
				for (name, value) in auth.get_request_headers(url).await? {
					if headers.get(&name).is_some() {
						continue;
					}
					headers.append(&name, &value);
				}
			}
		}
		Ok(headers)
	}
}

/// `getDefaultHeaders()`.
fn default_headers() -> Map<String, Value> {
	let version = format!("google-genai-sdk/{SDK_VERSION} {}", user_agent_extra());
	let mut headers = Map::new();
	headers.insert(USER_AGENT_HEADER.into(), json!(version));
	headers.insert(GOOGLE_API_CLIENT_HEADER.into(), json!(version));
	headers.insert(CONTENT_TYPE_HEADER.into(), json!("application/json"));
	headers
}

/// `patchHttpOptions(base, request)`: objects merge one level deep, other defined values replace.
fn patch_http_options(base: &Map<String, Value>, request: &Value) -> Map<String, Value> {
	let mut patched = base.clone();
	let Some(request) = request.as_object() else {
		return patched;
	};
	for (key, value) in request {
		match value {
			Value::Object(value) => {
				let mut merged = patched.get(key).and_then(Value::as_object).cloned().unwrap_or_default();
				merged.extend(value.clone());
				patched.insert(key.clone(), Value::Object(merged));
			}
			Value::Null => {
				let merged = patched.get(key).and_then(Value::as_object).cloned().unwrap_or_default();
				patched.insert(key.clone(), Value::Object(merged));
			}
			value => {
				patched.insert(key.clone(), value.clone());
			}
		}
	}
	patched
}

/// `url.searchParams.set(name, value)`.
fn set_search_param(url: &mut url::Url, name: &str, value: &str) {
	let mut pairs: Vec<(String, String)> = url.query_pairs().into_owned().collect();
	match pairs.iter().position(|(key, _)| key == name) {
		Some(index) => {
			pairs[index].1 = value.to_string();
			let mut seen = false;
			pairs.retain(|(key, _)| {
				if key != name {
					return true;
				}
				let keep = !seen;
				seen = true;
				keep
			});
		}
		None => pairs.push((name.to_string(), value.to_string())),
	}
	url.query_pairs_mut().clear().extend_pairs(pairs);
}

/// `includeExtraBodyToRequestInit`: deep-merges `extraBody` into a JSON object body.
fn include_extra_body(body: String, extra_body: Option<&Value>) -> String {
	let Some(Value::Object(extra_body)) = extra_body else {
		return body;
	};
	if extra_body.is_empty() {
		return body;
	}
	let current = if body.is_empty() {
		Map::new()
	} else {
		match serde_json::from_str::<Value>(&body) {
			Ok(Value::Object(object)) => object,
			_ => return body,
		}
	};
	json_stringify(&Value::Object(deep_merge(current, extra_body)))
}

fn deep_merge(target: Map<String, Value>, source: &Map<String, Value>) -> Map<String, Value> {
	let mut output = target;
	for (key, source_value) in source {
		let merged = match (source_value, output.get(key)) {
			(Value::Object(source_object), Some(Value::Object(target_object))) => {
				Value::Object(deep_merge(target_object.clone(), source_object))
			}
			_ => source_value.clone(),
		};
		output.insert(key.clone(), merged);
	}
	output
}

/// `apiCall` without `retryOptions`: one fetch whose signal aborts on the caller's signal or after `timeout` ms.
async fn api_call(
	url: &str,
	body: String,
	headers: Headers,
	attempt_signal: Option<AbortSignal>,
) -> PiResult<HttpResponse> {
	let mut request = HttpRequest::new("POST", url);
	request.headers = headers.into_index_map();
	request.body = Some(body.into());
	request.signal = attempt_signal;
	pi_host::fetch(request, None).await
}

/// `createAttemptSignal(timeout, callerSignal)`.
fn create_attempt_signal(timeout: Option<f64>, caller_signal: Option<AbortSignal>) -> Option<AbortSignal> {
	let timeout = timeout.filter(|timeout| *timeout > 0.0);
	if timeout.is_none() && caller_signal.is_none() {
		return None;
	}
	let controller = AbortController::new();
	if let Some(timeout) = timeout {
		let controller = controller.clone();
		pi_host::spawn_local(async move {
			pi_host::current().sleep(timeout).await;
			controller.abort();
		});
	}
	if let Some(caller_signal) = caller_signal {
		if caller_signal.aborted() {
			controller.abort();
		} else {
			let controller = controller.clone();
			caller_signal.add_listener(move |_| controller.abort());
		}
	}
	Some(controller.signal())
}

/// `throwErrorIfNotOK(response)`.
async fn throw_error_if_not_ok(response: HttpResponse) -> PiResult<HttpResponse> {
	if response.ok() {
		return Ok(response);
	}
	let status = response.status;
	let status_text = response.status_text.clone();
	let is_json = response
		.header("content-type")
		.is_some_and(|value| value.contains("application/json"));
	let error_body = if is_json {
		let mut body = response.json().await?;
		js_key_order(&mut body);
		body
	} else {
		let text = response.text().await?;
		json!({ "error": { "message": text, "code": status, "status": status_text } })
	};
	let message = json_stringify(&error_body);
	if (400..600).contains(&status) {
		return Err(api_error(message, status));
	}
	Err(PiError::new(message))
}

/// `new ApiError({ message, status })`.
fn api_error(message: String, status: u16) -> PiError {
	PiError::named("ApiError", message).with_status(status)
}

/// `processStreamResponse(response)`: yields each `data:` event parsed as JSON, with the response headers.
fn process_stream_response(
	response: HttpResponse,
	attempt_signal: Option<AbortSignal>,
) -> LocalBoxStream<'static, PiResult<(Value, IndexMap<String, String>)>> {
	let headers = response.headers.clone();
	let mut body = response.body;
	async_stream(move |yield_| async move {
		let mut decoder = Utf8StreamDecoder::default();
		let mut buffer = String::new();
		loop {
			let chunk = match body.next().await {
				Some(Ok(chunk)) => chunk,
				Some(Err(error)) => {
					yield_(Err(error)).await;
					return;
				}
				None => {
					// The host ends an aborted body; `reader.read()` rejects with the signal's reason instead.
					if let Some(reason) = attempt_signal.as_ref().and_then(AbortSignal::reason) {
						yield_(Err(reason)).await;
						return;
					}
					if !js_trim(&buffer).is_empty() {
						yield_(Err(PiError::new("Incomplete JSON segment at the end"))).await;
					}
					return;
				}
			};
			let chunk_string = decoder.decode(&chunk);
			if let Some(error) = chunk_api_error(&chunk_string) {
				yield_(Err(error)).await;
				return;
			}
			buffer.push_str(&chunk_string);
			while let Some((index, length)) = next_delimiter(&buffer) {
				let event = buffer[..index].to_string();
				buffer.drain(..index + length);
				let trimmed = js_trim(&event);
				let Some(data) = trimmed.strip_prefix("data:") else {
					continue;
				};
				let parsed = json_parse(js_trim(data)).map(|json| (json, headers.clone()));
				let failed = parsed.is_err();
				yield_(parsed).await;
				if failed {
					return;
				}
			}
		}
	})
}

/// The SDK parses each raw network chunk and throws an `ApiError` when it is an error object with a 4xx/5xx code.
fn chunk_api_error(chunk_string: &str) -> Option<PiError> {
	let Ok(Value::Object(chunk_json)) = serde_json::from_str::<Value>(chunk_string) else {
		return None;
	};
	let error = chunk_json.get("error")?.as_object()?;
	let code = match error.get("code")? {
		Value::Number(code) => code.as_f64()?,
		Value::String(code) => string_to_number(code),
		_ => return None,
	};
	if !(400.0..600.0).contains(&code) {
		return None;
	}
	let mut chunk_json = Value::Object(chunk_json.clone());
	js_key_order(&mut chunk_json);
	let status = error.get("status").map(js_string).unwrap_or_else(|| "undefined".into());
	let message = format!("got status: {status}. {}", json_stringify(&chunk_json));
	Some(api_error(message, code as u16))
}

/// The earliest of `"\n\n"`, `"\r\r"`, `"\r\n\r\n"` (the first listed wins a tie).
fn next_delimiter(buffer: &str) -> Option<(usize, usize)> {
	let mut best: Option<(usize, usize)> = None;
	for delimiter in ["\n\n", "\r\r", "\r\n\r\n"] {
		if let Some(index) = buffer.find(delimiter)
			&& best.is_none_or(|(best_index, _)| index < best_index)
		{
			best = Some((index, delimiter.len()));
		}
	}
	best
}

/// `new TextDecoder("utf-8").decode(chunk, { stream: true })`.
#[derive(Default)]
struct Utf8StreamDecoder {
	pending: Vec<u8>,
	started: bool,
}

impl Utf8StreamDecoder {
	fn decode(&mut self, chunk: &[u8]) -> String {
		self.pending.extend_from_slice(chunk);
		let mut out = String::new();
		let mut rest: &[u8] = &self.pending;
		loop {
			match std::str::from_utf8(rest) {
				Ok(valid) => {
					out.push_str(valid);
					rest = &[];
					break;
				}
				Err(error) => {
					let valid_up_to = error.valid_up_to();
					out.push_str(std::str::from_utf8(&rest[..valid_up_to]).expect("valid_up_to marks valid UTF-8"));
					match error.error_len() {
						Some(length) => {
							out.push('\u{FFFD}');
							rest = &rest[valid_up_to + length..];
						}
						None => {
							rest = &rest[valid_up_to..];
							break;
						}
					}
				}
			}
		}
		self.pending = rest.to_vec();
		if !self.started && !out.is_empty() {
			self.started = true;
			if let Some(without_bom) = out.strip_prefix('\u{FEFF}') {
				return without_bom.to_string();
			}
		}
		out
	}
}

/// A `Headers` object: lowercase names in insertion order, `append` joins repeated names with `", "`.
#[derive(Default)]
struct Headers {
	entries: Vec<(String, String)>,
}

impl Headers {
	fn append(&mut self, name: &str, value: &str) {
		let name = name.to_ascii_lowercase();
		let value = value.trim_matches([' ', '\t', '\n', '\r']);
		match self.entries.iter_mut().find(|(key, _)| *key == name) {
			Some((_, existing)) => {
				existing.push_str(", ");
				existing.push_str(value);
			}
			None => self.entries.push((name, value.to_string())),
		}
	}

	fn get(&self, name: &str) -> Option<&str> {
		let name = name.to_ascii_lowercase();
		self.entries
			.iter()
			.find(|(key, _)| *key == name)
			.map(|(_, value)| value.as_str())
	}

	fn into_index_map(self) -> IndexMap<String, String> {
		self.entries.into_iter().collect()
	}
}

/// `String(value)` for a header record value.
fn header_value_string(value: &Value) -> String {
	js_string(value)
}

/// `String(value)`.
pub(crate) fn js_string(value: &Value) -> String {
	match value {
		Value::String(value) => value.clone(),
		Value::Number(number) => number_to_string(number.as_f64().unwrap_or(f64::NAN)),
		Value::Bool(value) => value.to_string(),
		Value::Null => "null".into(),
		Value::Array(items) => items
			.iter()
			.map(|item| if item.is_null() { String::new() } else { js_string(item) })
			.collect::<Vec<_>>()
			.join(","),
		Value::Object(_) => "[object Object]".into(),
	}
}

/// JavaScript truthiness of a JSON value.
pub(crate) fn js_truthy(value: &Value) -> bool {
	match value {
		Value::Null => false,
		Value::Bool(value) => *value,
		Value::Number(number) => number.as_f64().is_some_and(|number| number != 0.0 && !number.is_nan()),
		Value::String(value) => !value.is_empty(),
		Value::Array(_) | Value::Object(_) => true,
	}
}

/// `maybeMoveToResponseJsonSchema(params)`.
fn maybe_move_to_response_json_schema(params: &mut Value) {
	let Some(config) = params.get_mut("config").and_then(Value::as_object_mut) else {
		return;
	};
	let Some(response_schema) = config.get("responseSchema").filter(|schema| js_truthy(schema)) else {
		return;
	};
	if config.get("responseJsonSchema").is_some_and(js_truthy) {
		return;
	}
	if response_schema
		.as_object()
		.is_some_and(|schema| schema.contains_key("$schema"))
	{
		let schema = config
			.shift_remove("responseSchema")
			.expect("responseSchema was just read");
		config.insert("responseJsonSchema".into(), schema);
	}
}

// ---------------------------------------------------------------------------------------------------------------
// Converters. Each one is the SDK's generated `fooToMldev` / `fooToVertex` / `fooFromMldev`, field by field in the
// SDK's order: `getValueByPath(from, [key]) != null` copies (or converts) a field, `!== undefined` rejects a field the
// other backend owns.
// ---------------------------------------------------------------------------------------------------------------

type Converter = fn(&Value) -> PiResult<Value>;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Backend {
	Mldev,
	Vertex,
}

struct Convert<'a> {
	from: &'a Value,
	to: Map<String, Value>,
}

impl<'a> Convert<'a> {
	fn new(from: &'a Value) -> Self {
		Self { from, to: Map::new() }
	}

	fn get(&self, key: &str) -> Option<&'a Value> {
		self.from.as_object()?.get(key).filter(|value| !value.is_null())
	}

	fn copy(&mut self, key: &str) -> PiResult<()> {
		self.copy_to(key, &[key])
	}

	fn copy_to(&mut self, key: &str, path: &[&str]) -> PiResult<()> {
		if let Some(value) = self.get(key) {
			set_value_by_path(&mut self.to, path, value.clone())?;
		}
		Ok(())
	}

	fn convert(&mut self, key: &str, convert: impl FnOnce(&Value) -> PiResult<Value>) -> PiResult<()> {
		if let Some(value) = self.get(key) {
			let converted = convert(value)?;
			set_value_by_path(&mut self.to, &[key], converted)?;
		}
		Ok(())
	}

	fn convert_list(&mut self, key: &str, convert: Converter) -> PiResult<()> {
		if let Some(value) = self.get(key) {
			let converted = map_list(value, convert)?;
			set_value_by_path(&mut self.to, &[key], converted)?;
		}
		Ok(())
	}

	/// A field only the other backend accepts.
	fn reject(&self, key: &str, backend: Backend) -> PiResult<()> {
		if self.from.as_object().is_some_and(|object| object.contains_key(key)) {
			return Err(unsupported(key, backend));
		}
		Ok(())
	}

	fn done(self) -> Value {
		Value::Object(self.to)
	}
}

/// The SDK error for a field the current backend does not accept.
fn unsupported(key: &str, backend: Backend) -> PiError {
	PiError::new(match backend {
		Backend::Mldev => format!(
			"{key} parameter is only supported in Gemini Enterprise Agent Platform mode, not in Gemini Developer API mode."
		),
		Backend::Vertex => format!(
			"{key} parameter is only supported in Gemini Developer API mode, not in Gemini Enterprise Agent Platform mode."
		),
	})
}

/// `if (Array.isArray(list)) list = list.map(convert)`.
fn map_list(value: &Value, convert: Converter) -> PiResult<Value> {
	match value {
		Value::Array(items) => Ok(Value::Array(items.iter().map(convert).collect::<PiResult<_>>()?)),
		other => Ok(other.clone()),
	}
}

fn identity(value: &Value) -> PiResult<Value> {
	Ok(value.clone())
}

/// `setValueByPath(data, keys, value)` for plain keys.
fn set_value_by_path(data: &mut Map<String, Value>, keys: &[&str], value: Value) -> PiResult<()> {
	let (last, parents) = keys.split_last().expect("setValueByPath needs a key");
	let mut data = data;
	for key in parents {
		if !data
			.get(*key)
			.is_some_and(|value| value.is_object() && js_truthy(value))
		{
			data.insert((*key).to_string(), Value::Object(Map::new()));
		}
		data = data
			.get_mut(*key)
			.and_then(Value::as_object_mut)
			.expect("parent was just made an object");
	}
	let Some(existing) = data.get_mut(*last) else {
		if *last == "_self"
			&& let Value::Object(value) = value
		{
			data.extend(value);
			return Ok(());
		}
		data.insert((*last).to_string(), value);
		return Ok(());
	};
	let empty_object = match &value {
		Value::Object(object) => object.is_empty(),
		Value::Array(items) => items.is_empty(),
		_ => false,
	};
	if !js_truthy(&value) || empty_object || value == *existing && !value.is_object() && !value.is_array() {
		return Ok(());
	}
	match (existing, value) {
		(Value::Object(existing), Value::Object(value)) => {
			existing.extend(value);
			Ok(())
		}
		_ => Err(PiError::new(format!(
			"Cannot set value for an existing key. Key: {last}"
		))),
	}
}

fn generate_content_parameters_to_mldev(client: &ApiClient, from: &Value) -> PiResult<Map<String, Value>> {
	let mut c = Convert::new(from);
	if let Some(model) = c.get("model") {
		set_value_by_path(&mut c.to, &["_url", "model"], json!(t_model(client, model)?))?;
	}
	if let Some(contents) = c.get("contents") {
		let contents = t_contents(contents)?
			.iter()
			.map(content_to_mldev)
			.collect::<PiResult<Vec<_>>>()?;
		set_value_by_path(&mut c.to, &["contents"], Value::Array(contents))?;
	}
	if let Some(config) = c.get("config") {
		let generation_config = generate_content_config_to_mldev(client, config, &mut c.to)?;
		set_value_by_path(&mut c.to, &["generationConfig"], generation_config)?;
	}
	Ok(c.to)
}

fn generate_content_parameters_to_vertex(client: &ApiClient, from: &Value) -> PiResult<Map<String, Value>> {
	let mut c = Convert::new(from);
	if let Some(model) = c.get("model") {
		set_value_by_path(&mut c.to, &["_url", "model"], json!(t_model(client, model)?))?;
	}
	if let Some(contents) = c.get("contents") {
		let contents = t_contents(contents)?
			.iter()
			.map(content_to_vertex)
			.collect::<PiResult<Vec<_>>>()?;
		set_value_by_path(&mut c.to, &["contents"], Value::Array(contents))?;
	}
	if let Some(config) = c.get("config") {
		let generation_config = generate_content_config_to_vertex(client, config, &mut c.to)?;
		set_value_by_path(&mut c.to, &["generationConfig"], generation_config)?;
	}
	Ok(c.to)
}

/// The `generationConfig` fields both backends copy unchanged, in SDK order, up to `responseJsonSchema`.
const SAMPLING_FIELDS: [&str; 13] = [
	"temperature",
	"topP",
	"topK",
	"candidateCount",
	"maxOutputTokens",
	"stopSequences",
	"responseLogprobs",
	"logprobs",
	"presencePenalty",
	"frequencyPenalty",
	"seed",
	"responseMimeType",
	"responseSchema",
];

/// Copies `from[key]` into the request body (the converter's `parentObject`).
fn set_parent(parent: &mut Map<String, Value>, key: &str, value: Value) -> PiResult<()> {
	set_value_by_path(parent, &[key], value)
}

fn copy_sampling_fields(c: &mut Convert) -> PiResult<()> {
	for key in SAMPLING_FIELDS {
		if key == "responseSchema" {
			c.convert(key, t_schema)?;
		} else {
			c.copy(key)?;
		}
	}
	c.convert("responseJsonSchema", identity)
}

fn generate_content_config_to_mldev(
	client: &ApiClient,
	from: &Value,
	parent: &mut Map<String, Value>,
) -> PiResult<Value> {
	let mut c = Convert::new(from);
	if let Some(service_tier) = c.get("serviceTier") {
		set_parent(parent, "serviceTier", service_tier.clone())?;
	}
	if let Some(system_instruction) = c.get("systemInstruction") {
		set_parent(
			parent,
			"systemInstruction",
			content_to_mldev(&t_content(system_instruction)?)?,
		)?;
	}
	copy_sampling_fields(&mut c)?;
	c.reject("routingConfig", Backend::Mldev)?;
	c.reject("modelSelectionConfig", Backend::Mldev)?;
	if let Some(safety_settings) = c.get("safetySettings") {
		set_parent(
			parent,
			"safetySettings",
			map_list(safety_settings, safety_setting_to_mldev)?,
		)?;
	}
	if let Some(tools) = c.get("tools") {
		let tools = t_tools(tools)?
			.iter()
			.map(|tool| tool_to_mldev(&t_tool(tool)?))
			.collect::<PiResult<_>>()?;
		set_parent(parent, "tools", Value::Array(tools))?;
	}
	if let Some(tool_config) = c.get("toolConfig") {
		set_parent(parent, "toolConfig", tool_config_to_mldev(tool_config)?)?;
	}
	c.reject("labels", Backend::Mldev)?;
	if let Some(cached_content) = c.get("cachedContent") {
		set_parent(
			parent,
			"cachedContent",
			json!(t_cached_content_name(client, cached_content)?),
		)?;
	}
	c.copy("responseModalities")?;
	c.copy("mediaResolution")?;
	c.convert("speechConfig", t_speech_config)?;
	c.reject("audioTimestamp", Backend::Mldev)?;
	c.copy("thinkingConfig")?;
	c.copy("audioTranscriptionConfig")?;
	c.convert("imageConfig", image_config_to_mldev)?;
	c.copy("enableEnhancedCivicAnswers")?;
	c.reject("modelArmorConfig", Backend::Mldev)?;
	Ok(c.done())
}

fn generate_content_config_to_vertex(
	client: &ApiClient,
	from: &Value,
	parent: &mut Map<String, Value>,
) -> PiResult<Value> {
	let mut c = Convert::new(from);
	if let Some(service_tier) = c.get("serviceTier") {
		set_parent(parent, "serviceTier", service_tier.clone())?;
	}
	if let Some(system_instruction) = c.get("systemInstruction") {
		set_parent(
			parent,
			"systemInstruction",
			content_to_vertex(&t_content(system_instruction)?)?,
		)?;
	}
	copy_sampling_fields(&mut c)?;
	c.copy("routingConfig")?;
	c.copy_to("modelSelectionConfig", &["modelConfig"])?;
	if let Some(safety_settings) = c.get("safetySettings") {
		set_parent(parent, "safetySettings", safety_settings.clone())?;
	}
	if let Some(tools) = c.get("tools") {
		let tools = t_tools(tools)?
			.iter()
			.map(|tool| tool_to_vertex(&t_tool(tool)?))
			.collect::<PiResult<_>>()?;
		set_parent(parent, "tools", Value::Array(tools))?;
	}
	if let Some(tool_config) = c.get("toolConfig") {
		set_parent(parent, "toolConfig", tool_config_to_vertex(tool_config)?)?;
	}
	if let Some(labels) = c.get("labels") {
		set_parent(parent, "labels", labels.clone())?;
	}
	if let Some(cached_content) = c.get("cachedContent") {
		set_parent(
			parent,
			"cachedContent",
			json!(t_cached_content_name(client, cached_content)?),
		)?;
	}
	c.copy("responseModalities")?;
	c.copy("mediaResolution")?;
	c.convert("speechConfig", |value| {
		speech_config_to_vertex(&t_speech_config(value)?)
	})?;
	c.copy("audioTimestamp")?;
	c.copy("thinkingConfig")?;
	c.copy("audioTranscriptionConfig")?;
	c.convert("imageConfig", image_config_to_vertex)?;
	c.reject("enableEnhancedCivicAnswers", Backend::Vertex)?;
	if let Some(model_armor_config) = c.get("modelArmorConfig") {
		set_parent(parent, "modelArmorConfig", model_armor_config.clone())?;
	}
	Ok(c.done())
}

fn content_to_mldev(from: &Value) -> PiResult<Value> {
	let mut c = Convert::new(from);
	c.convert_list("parts", part_to_mldev)?;
	c.copy("role")?;
	Ok(c.done())
}

fn content_to_vertex(from: &Value) -> PiResult<Value> {
	let mut c = Convert::new(from);
	c.convert_list("parts", part_to_vertex)?;
	c.copy("role")?;
	Ok(c.done())
}

fn part_to_mldev(from: &Value) -> PiResult<Value> {
	let mut c = Convert::new(from);
	for key in [
		"mediaResolution",
		"toolCall",
		"toolResponse",
		"audioTranscription",
		"codeExecutionResult",
		"executableCode",
	] {
		c.copy(key)?;
	}
	c.convert("fileData", file_data_to_mldev)?;
	c.convert("functionCall", function_call_to_mldev)?;
	c.copy("functionResponse")?;
	c.convert("inlineData", blob_to_mldev)?;
	for key in [
		"text",
		"thought",
		"thoughtSignature",
		"videoMetadata",
		"partMetadata",
		"mediaProcessing",
	] {
		c.copy(key)?;
	}
	Ok(c.done())
}

fn part_to_vertex(from: &Value) -> PiResult<Value> {
	let mut c = Convert::new(from);
	c.copy("mediaResolution")?;
	c.reject("toolCall", Backend::Vertex)?;
	c.reject("toolResponse", Backend::Vertex)?;
	for key in [
		"audioTranscription",
		"codeExecutionResult",
		"executableCode",
		"fileData",
		"functionCall",
		"functionResponse",
		"inlineData",
		"text",
		"thought",
		"thoughtSignature",
		"videoMetadata",
	] {
		c.copy(key)?;
	}
	c.reject("partMetadata", Backend::Vertex)?;
	c.copy("mediaProcessing")?;
	Ok(c.done())
}

fn file_data_to_mldev(from: &Value) -> PiResult<Value> {
	let mut c = Convert::new(from);
	c.reject("displayName", Backend::Mldev)?;
	c.copy("fileUri")?;
	c.copy("mimeType")?;
	Ok(c.done())
}

fn function_call_to_mldev(from: &Value) -> PiResult<Value> {
	let mut c = Convert::new(from);
	c.copy("args")?;
	c.copy("id")?;
	c.copy("name")?;
	c.reject("partialArgs", Backend::Mldev)?;
	c.reject("willContinue", Backend::Mldev)?;
	Ok(c.done())
}

fn blob_to_mldev(from: &Value) -> PiResult<Value> {
	let mut c = Convert::new(from);
	c.copy("data")?;
	c.reject("displayName", Backend::Mldev)?;
	c.copy("mimeType")?;
	Ok(c.done())
}

fn tool_to_mldev(from: &Value) -> PiResult<Value> {
	let mut c = Convert::new(from);
	c.reject("retrieval", Backend::Mldev)?;
	c.convert("googleMaps", google_maps_to_mldev)?;
	c.convert_list("mcpServers", identity)?;
	c.copy("codeExecution")?;
	c.copy("computerUse")?;
	c.reject("enterpriseWebSearch", Backend::Mldev)?;
	c.reject("exaAiSearch", Backend::Mldev)?;
	c.convert_list("functionDeclarations", identity)?;
	c.convert("googleSearch", google_search_to_mldev)?;
	c.copy("googleSearchRetrieval")?;
	c.reject("parallelAiSearch", Backend::Mldev)?;
	c.copy("urlContext")?;
	c.copy("fileSearch")?;
	Ok(c.done())
}

fn tool_to_vertex(from: &Value) -> PiResult<Value> {
	let mut c = Convert::new(from);
	c.copy("retrieval")?;
	c.copy("googleMaps")?;
	c.convert_list("mcpServers", mcp_server_to_vertex)?;
	c.copy("codeExecution")?;
	c.convert("computerUse", computer_use_to_vertex)?;
	c.copy("enterpriseWebSearch")?;
	c.copy("exaAiSearch")?;
	c.convert_list("functionDeclarations", identity)?;
	c.copy("googleSearch")?;
	c.copy("googleSearchRetrieval")?;
	c.copy("parallelAiSearch")?;
	c.copy("urlContext")?;
	c.reject("fileSearch", Backend::Vertex)?;
	Ok(c.done())
}

fn google_maps_to_mldev(from: &Value) -> PiResult<Value> {
	let mut c = Convert::new(from);
	c.convert("authConfig", auth_config_to_mldev)?;
	c.copy("enableWidget")?;
	c.reject("groundingTypes", Backend::Mldev)?;
	Ok(c.done())
}

fn auth_config_to_mldev(from: &Value) -> PiResult<Value> {
	let mut c = Convert::new(from);
	c.copy("apiKey")?;
	for key in [
		"apiKeyConfig",
		"authType",
		"googleServiceAccountConfig",
		"httpBasicAuthConfig",
		"oauthConfig",
		"oidcConfig",
	] {
		c.reject(key, Backend::Mldev)?;
	}
	Ok(c.done())
}

fn google_search_to_mldev(from: &Value) -> PiResult<Value> {
	let mut c = Convert::new(from);
	c.reject("blockingConfidence", Backend::Mldev)?;
	c.reject("excludeDomains", Backend::Mldev)?;
	c.copy("searchTypes")?;
	c.copy("timeRangeFilter")?;
	Ok(c.done())
}

fn mcp_server_to_vertex(from: &Value) -> PiResult<Value> {
	let c = Convert::new(from);
	c.reject("name", Backend::Vertex)?;
	c.reject("streamableHttpTransport", Backend::Vertex)?;
	Ok(c.done())
}

fn computer_use_to_vertex(from: &Value) -> PiResult<Value> {
	let mut c = Convert::new(from);
	c.copy("enablePromptInjectionDetection")?;
	c.copy("environment")?;
	c.copy("excludedPredefinedFunctions")?;
	c.reject("disabledSafetyPolicies", Backend::Vertex)?;
	Ok(c.done())
}

fn tool_config_to_mldev(from: &Value) -> PiResult<Value> {
	let mut c = Convert::new(from);
	c.convert("functionCallingConfig", function_calling_config_to_mldev)?;
	c.copy("retrievalConfig")?;
	c.copy("includeServerSideToolInvocations")?;
	Ok(c.done())
}

fn function_calling_config_to_mldev(from: &Value) -> PiResult<Value> {
	let mut c = Convert::new(from);
	c.copy("allowedFunctionNames")?;
	c.copy("mode")?;
	c.reject("streamFunctionCallArguments", Backend::Mldev)?;
	Ok(c.done())
}

fn tool_config_to_vertex(from: &Value) -> PiResult<Value> {
	let mut c = Convert::new(from);
	c.copy("functionCallingConfig")?;
	c.copy("retrievalConfig")?;
	c.reject("includeServerSideToolInvocations", Backend::Vertex)?;
	Ok(c.done())
}

fn safety_setting_to_mldev(from: &Value) -> PiResult<Value> {
	let mut c = Convert::new(from);
	c.copy("category")?;
	c.reject("method", Backend::Mldev)?;
	c.copy("threshold")?;
	Ok(c.done())
}

fn image_config_to_mldev(from: &Value) -> PiResult<Value> {
	let mut c = Convert::new(from);
	c.copy("aspectRatio")?;
	c.copy("imageSize")?;
	for key in [
		"personGeneration",
		"outputMimeType",
		"outputCompressionQuality",
		"imageOutputOptions",
		"prominentPeople",
	] {
		c.reject(key, Backend::Mldev)?;
	}
	Ok(c.done())
}

fn image_config_to_vertex(from: &Value) -> PiResult<Value> {
	let mut c = Convert::new(from);
	c.copy("aspectRatio")?;
	c.copy("imageSize")?;
	c.copy("personGeneration")?;
	c.copy_to("outputMimeType", &["imageOutputOptions", "mimeType"])?;
	c.copy_to(
		"outputCompressionQuality",
		&["imageOutputOptions", "compressionQuality"],
	)?;
	c.copy("imageOutputOptions")?;
	c.copy("prominentPeople")?;
	Ok(c.done())
}

fn speech_config_to_vertex(from: &Value) -> PiResult<Value> {
	let mut c = Convert::new(from);
	c.convert("voiceConfig", voice_config_to_vertex)?;
	c.copy("languageCode")?;
	c.convert("multiSpeakerVoiceConfig", multi_speaker_voice_config_to_vertex)?;
	Ok(c.done())
}

fn voice_config_to_vertex(from: &Value) -> PiResult<Value> {
	let mut c = Convert::new(from);
	c.convert("replicatedVoiceConfig", replicated_voice_config_to_vertex)?;
	c.copy("prebuiltVoiceConfig")?;
	Ok(c.done())
}

fn multi_speaker_voice_config_to_vertex(from: &Value) -> PiResult<Value> {
	let mut c = Convert::new(from);
	c.convert_list("speakerVoiceConfigs", speaker_voice_config_to_vertex)?;
	Ok(c.done())
}

fn speaker_voice_config_to_vertex(from: &Value) -> PiResult<Value> {
	let mut c = Convert::new(from);
	c.copy("speaker")?;
	c.convert("voiceConfig", voice_config_to_vertex)?;
	Ok(c.done())
}

fn replicated_voice_config_to_vertex(from: &Value) -> PiResult<Value> {
	let mut c = Convert::new(from);
	c.copy("mimeType")?;
	c.copy("voiceSampleAudio")?;
	c.reject("consentAudio", Backend::Vertex)?;
	c.reject("voiceConsentSignature", Backend::Vertex)?;
	Ok(c.done())
}

fn generate_content_response_from_mldev(from: &Value) -> PiResult<Map<String, Value>> {
	let mut c = Convert::new(from);
	c.copy("sdkHttpResponse")?;
	c.convert_list("candidates", candidate_from_mldev)?;
	for key in [
		"modelVersion",
		"promptFeedback",
		"responseId",
		"usageMetadata",
		"modelStatus",
	] {
		c.copy(key)?;
	}
	Ok(c.to)
}

fn generate_content_response_from_vertex(from: &Value) -> PiResult<Map<String, Value>> {
	let mut c = Convert::new(from);
	c.copy("sdkHttpResponse")?;
	c.convert_list("candidates", identity)?;
	for key in [
		"createTime",
		"modelVersion",
		"promptFeedback",
		"responseId",
		"usageMetadata",
	] {
		c.copy(key)?;
	}
	Ok(c.to)
}

fn candidate_from_mldev(from: &Value) -> PiResult<Value> {
	let mut c = Convert::new(from);
	c.copy("content")?;
	c.convert("citationMetadata", citation_metadata_from_mldev)?;
	for key in [
		"tokenCount",
		"finishReason",
		"groundingMetadata",
		"avgLogprobs",
		"index",
		"logprobsResult",
	] {
		c.copy(key)?;
	}
	c.convert_list("safetyRatings", identity)?;
	c.copy("urlContextMetadata")?;
	Ok(c.done())
}

fn citation_metadata_from_mldev(from: &Value) -> PiResult<Value> {
	let mut c = Convert::new(from);
	if let Some(citations) = c.get("citationSources") {
		set_value_by_path(&mut c.to, &["citations"], map_list(citations, identity)?)?;
	}
	Ok(c.done())
}

/// `tModel(apiClient, model)`.
fn t_model(client: &ApiClient, model: &Value) -> PiResult<String> {
	let Some(model) = model.as_str().filter(|model| !model.is_empty()) else {
		return Err(PiError::new("model is required and must be a string"));
	};
	if model.contains("..") || model.contains('?') || model.contains('&') {
		return Err(PiError::new("invalid model parameter"));
	}
	if client.vertexai {
		if model.starts_with("publishers/") || model.starts_with("projects/") || model.starts_with("models/") {
			return Ok(model.to_string());
		}
		if model.contains('/') {
			let mut parts = model.split('/');
			let publisher = parts.next().unwrap_or_default();
			let name = parts.next().unwrap_or_default();
			return Ok(format!("publishers/{publisher}/models/{name}"));
		}
		return Ok(format!("publishers/google/models/{model}"));
	}
	if model.starts_with("models/") || model.starts_with("tunedModels/") {
		return Ok(model.to_string());
	}
	Ok(format!("models/{model}"))
}

fn is_content(value: &Value) -> bool {
	value.get("parts").is_some_and(Value::is_array)
}

fn is_function_call_or_response_part(value: &Value) -> bool {
	value
		.as_object()
		.is_some_and(|object| object.contains_key("functionCall") || object.contains_key("functionResponse"))
}

/// `tContents(origin)`.
fn t_contents(origin: &Value) -> PiResult<Vec<Value>> {
	let items = match origin {
		Value::Null => return Err(PiError::new("contents are required")),
		Value::Array(items) if items.is_empty() => return Err(PiError::new("contents are required")),
		Value::Array(items) => items,
		other => {
			if is_function_call_or_response_part(other) {
				return Err(PiError::new(
					"To specify functionCall or functionResponse parts, please wrap them in a Content object, specifying the role for them",
				));
			}
			return Ok(vec![t_content(other)?]);
		}
	};
	let is_content_array = is_content(&items[0]);
	let mut result = Vec::new();
	let mut accumulated_parts = Vec::new();
	for item in items {
		let item_is_content = is_content(item);
		if item_is_content != is_content_array {
			return Err(PiError::new(
				"Mixing Content and Parts is not supported, please group the parts into a the appropriate Content objects and specify the roles for them",
			));
		}
		if item_is_content {
			result.push(item.clone());
		} else if is_function_call_or_response_part(item) {
			return Err(PiError::new(
				"To specify functionCall or functionResponse parts, please wrap them, and any other parts, in Content objects as appropriate, specifying the role for them",
			));
		} else {
			accumulated_parts.push(item.clone());
		}
	}
	if !is_content_array {
		result.push(json!({ "role": "user", "parts": t_parts(&Value::Array(accumulated_parts))? }));
	}
	Ok(result)
}

/// `tContent(origin)`.
fn t_content(origin: &Value) -> PiResult<Value> {
	if origin.is_null() {
		return Err(PiError::new("ContentUnion is required"));
	}
	if is_content(origin) {
		return Ok(origin.clone());
	}
	Ok(json!({ "role": "user", "parts": t_parts(origin)? }))
}

/// `tParts(origin)`.
fn t_parts(origin: &Value) -> PiResult<Value> {
	match origin {
		Value::Null => Err(PiError::new("PartListUnion is required")),
		Value::Array(items) if items.is_empty() => Err(PiError::new("PartListUnion is required")),
		Value::Array(items) => Ok(Value::Array(items.iter().map(t_part).collect::<PiResult<_>>()?)),
		other => Ok(Value::Array(vec![t_part(other)?])),
	}
}

/// `tPart(origin)`.
fn t_part(origin: &Value) -> PiResult<Value> {
	match origin {
		Value::Null => Err(PiError::new("PartUnion is required")),
		Value::Object(_) | Value::Array(_) => Ok(origin.clone()),
		Value::String(text) => Ok(json!({ "text": text })),
		Value::Number(_) => Err(PiError::new("Unsupported part type: number")),
		Value::Bool(_) => Err(PiError::new("Unsupported part type: boolean")),
	}
}

/// `tTools(tools)`.
fn t_tools(tools: &Value) -> PiResult<&Vec<Value>> {
	tools
		.as_array()
		.ok_or_else(|| PiError::new("tools is required and must be an array of Tools"))
}

/// `tTool(tool)`: plain JSON schemas become Gemini schemas; `$schema` ones move to `parametersJsonSchema`.
fn t_tool(tool: &Value) -> PiResult<Value> {
	let mut tool = tool.clone();
	let Some(declarations) = tool.get_mut("functionDeclarations").and_then(Value::as_array_mut) else {
		return Ok(tool);
	};
	for declaration in declarations.iter_mut().filter_map(Value::as_object_mut) {
		for (key, json_key) in [
			("parameters", "parametersJsonSchema"),
			("response", "responseJsonSchema"),
		] {
			let Some(schema) = declaration.get(key).filter(|schema| js_truthy(schema)) else {
				continue;
			};
			if !schema.as_object().is_some_and(|schema| schema.contains_key("$schema")) {
				let processed = process_json_schema(schema)?;
				declaration.insert(key.into(), processed);
			} else if !declaration.get(json_key).is_some_and(js_truthy) {
				let schema = declaration.shift_remove(key).expect("schema was just read");
				declaration.insert(json_key.into(), schema);
			}
		}
	}
	Ok(tool)
}

/// `tSchema(schema)`.
fn t_schema(schema: &Value) -> PiResult<Value> {
	process_json_schema(schema)
}

fn gemini_type(name: &str) -> &'static str {
	let upper = name.to_uppercase();
	TYPE_VALUES
		.into_iter()
		.find(|value| *value == upper)
		.unwrap_or("TYPE_UNSPECIFIED")
}

/// `flattenTypeArrayToAnyOf(typeList, resultingSchema)`.
fn flatten_type_array_to_any_of(type_list: &[Value], schema: &mut Map<String, Value>) {
	if type_list.iter().any(|value| value == "null") {
		schema.insert("nullable".into(), json!(true));
	}
	let without_null: Vec<String> = type_list
		.iter()
		.filter(|value| *value != "null")
		.map(js_string)
		.collect();
	if without_null.len() == 1 {
		schema.insert("type".into(), json!(gemini_type(&without_null[0])));
	} else {
		let any_of: Vec<Value> = without_null
			.iter()
			.map(|value| json!({ "type": gemini_type(value) }))
			.collect();
		schema.insert("anyOf".into(), Value::Array(any_of));
	}
}

/// `processJsonSchema(jsonSchema)`.
fn process_json_schema(json_schema: &Value) -> PiResult<Value> {
	let mut gen_ai_schema = Map::new();
	let mut json_schema = json_schema;
	if json_schema.get("type").is_some_and(js_truthy) && json_schema.get("anyOf").is_some_and(js_truthy) {
		return Err(PiError::new("type and anyOf cannot be both populated."));
	}
	if let Some(Value::Array(any_of)) = json_schema.get("anyOf")
		&& any_of.len() == 2
	{
		if any_of[0].get("type").is_some_and(|value| value == "null") {
			gen_ai_schema.insert("nullable".into(), json!(true));
			json_schema = &any_of[1];
		} else if any_of[1].get("type").is_some_and(|value| value == "null") {
			gen_ai_schema.insert("nullable".into(), json!(true));
			json_schema = &any_of[0];
		}
	}
	if let Some(Value::Array(types)) = json_schema.get("type") {
		flatten_type_array_to_any_of(types, &mut gen_ai_schema);
	}
	let Some(entries) = json_schema.as_object() else {
		return Ok(Value::Object(gen_ai_schema));
	};
	for (field_name, field_value) in entries {
		if field_value.is_null() {
			continue;
		}
		match field_name.as_str() {
			"type" => {
				if field_value == "null" {
					return Err(PiError::new(
						"type: null can not be the only possible type for the field.",
					));
				}
				if field_value.is_array() {
					continue;
				}
				gen_ai_schema.insert("type".into(), json!(gemini_type(&js_string(field_value))));
			}
			"items" => {
				gen_ai_schema.insert(field_name.clone(), process_json_schema(field_value)?);
			}
			"anyOf" => {
				let mut list = Vec::new();
				for item in field_value.as_array().into_iter().flatten() {
					if item.get("type").is_some_and(|value| value == "null") {
						gen_ai_schema.insert("nullable".into(), json!(true));
						continue;
					}
					list.push(process_json_schema(item)?);
				}
				gen_ai_schema.insert(field_name.clone(), Value::Array(list));
			}
			"properties" => {
				let mut properties = Map::new();
				for (key, value) in field_value.as_object().into_iter().flatten() {
					properties.insert(key.clone(), process_json_schema(value)?);
				}
				gen_ai_schema.insert(field_name.clone(), Value::Object(properties));
			}
			"additionalProperties" => {}
			_ => {
				gen_ai_schema.insert(field_name.clone(), field_value.clone());
			}
		}
	}
	Ok(Value::Object(gen_ai_schema))
}

/// `tSpeechConfig(speechConfig)`.
fn t_speech_config(speech_config: &Value) -> PiResult<Value> {
	match speech_config {
		Value::String(voice_name) => {
			Ok(json!({ "voiceConfig": { "prebuiltVoiceConfig": { "voiceName": voice_name } } }))
		}
		Value::Object(_) | Value::Array(_) | Value::Null => Ok(speech_config.clone()),
		Value::Number(_) => Err(PiError::new("Unsupported speechConfig type: number")),
		Value::Bool(_) => Err(PiError::new("Unsupported speechConfig type: boolean")),
	}
}

/// `tCachedContentName(apiClient, name)`.
fn t_cached_content_name(client: &ApiClient, name: &Value) -> PiResult<String> {
	let Some(name) = name.as_str() else {
		return Err(PiError::new("name must be a string"));
	};
	Ok(resource_name(client, name, "cachedContents"))
}

/// `resourceName(client, resourceName, resourcePrefix)`.
fn resource_name(client: &ApiClient, name: &str, prefix: &str) -> String {
	let should_append_prefix = !name.starts_with(&format!("{prefix}/")) && name.split('/').count() == 1;
	let project = client.project.as_deref().unwrap_or("undefined");
	let location = client.location.as_deref().unwrap_or("undefined");
	if client.vertexai {
		if name.starts_with("projects/") {
			return name.to_string();
		}
		if name.starts_with("locations/") {
			return format!("projects/{project}/{name}");
		}
		if name.starts_with(&format!("{prefix}/")) {
			return format!("projects/{project}/locations/{location}/{name}");
		}
		if should_append_prefix {
			return format!("projects/{project}/locations/{location}/{prefix}/{name}");
		}
		return name.to_string();
	}
	if should_append_prefix {
		return format!("{prefix}/{name}");
	}
	name.to_string()
}
