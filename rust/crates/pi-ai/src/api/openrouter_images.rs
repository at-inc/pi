//! Port of `packages/ai/src/api/openrouter-images.ts`.
//! PORT-STATUS: ported
//!
//! Image generation over OpenRouter's chat completions endpoint.
//!
//! The TS calls `client.chat.completions.create(params, { signal, timeout, maxRetries: 0 }).withResponse()` on the
//! `openai` SDK (7.19.0). `api/openai_client.rs` belongs to the OpenAI API shards and does not exist yet, so this
//! file reproduces the slice of SDK behavior that one non-streaming call with `maxRetries: 0` observes (see
//! [`OpenAiChatClient`]); swap it for the shared client once that lands. Differences from the TS:
//! - The response is read as JSON values, not JS objects: a property read on a non-object primitive gives
//!   `undefined` (strings are not indexed), a non-number token count reads as 0, and a non-string `id` is dropped.
//! - `X-Stainless-Runtime-Version` is `unknown` (there is no Node `process.version`), and on wasm32 the platform
//!   headers are the SDK's unknown-runtime set.
//! - Header values are not checked for the characters `Headers` rejects.
//! - `OPENAI_LOG` is not read, so nothing is logged at any level and an invalid level gives no warning.

use std::collections::HashSet;
use std::convert::Infallible;
use std::pin::pin;

use futures::future::{Either, select};
use indexmap::IndexMap;
use pi_host::js::json_stringify;
use pi_host::{AbortController, AbortSignal, HttpRequest, HttpResponse, LocalBoxFuture, PiError, PiResult};
use regex::Regex;
use serde_json::{Map, Value, json};

use crate::types::{
	AssistantImages, ImageContent, ImageModel, ImagesContext, ImagesOptions, ImagesStopReason, ModelInputModality,
	ProviderHeaders, ProviderResponse, TextContent, Usage, UsageCost, UserContent,
};
use crate::utils::error_body::{format_provider_error, normalize_provider_error};
use crate::utils::headers::{headers_to_record, provider_headers_to_record};
use crate::utils::identity::{ClientIdentityTarget, get_identity_headers};
use crate::utils::json_parse::json_parse;
use crate::utils::provider_retry::{ProviderRetryOptions, retry_provider_request};
use crate::utils::sanitize_unicode::sanitize_surrogates;
use crate::utils::text::js_trim;

/// Image generation over OpenRouter's chat completions endpoint.
pub fn generate_images(
	model: &ImageModel,
	context: &ImagesContext,
	options: Option<ImagesOptions>,
) -> LocalBoxFuture<'static, PiResult<AssistantImages>> {
	let mut output = AssistantImages {
		api: model.api.clone(),
		provider: model.provider.clone(),
		model: model.id.clone(),
		output: Vec::new(),
		response_id: None,
		usage: None,
		stop_reason: ImagesStopReason::Stop,
		error_message: None,
		timestamp: pi_host::now_ms(),
	};
	let (model, context) = (model.clone(), context.clone());
	let options = options.unwrap_or_default();
	Box::pin(async move {
		if let Err(error) = generate(&model, &context, &options, &mut output).await {
			output.stop_reason = if options.signal.as_ref().is_some_and(AbortSignal::aborted) {
				ImagesStopReason::Aborted
			} else {
				ImagesStopReason::Error
			};
			output.error_message = Some(format_provider_error(&normalize_provider_error(&error), None));
		}
		Ok(output)
	})
}

async fn generate(
	model: &ImageModel,
	context: &ImagesContext,
	options: &ImagesOptions,
	output: &mut AssistantImages,
) -> PiResult<()> {
	let Some(api_key) = options.api_key.as_deref().filter(|key| !key.is_empty()) else {
		return Err(PiError::new(format!("No API key for provider: {}", model.provider)));
	};
	let client = create_client(model, api_key, options)?;
	let mut params = build_params(model, context);
	if let Some(on_payload) = &options.on_payload
		&& let Some(next_params) = on_payload(params.clone(), model).await?
	{
		params = next_params;
	}
	let (status, headers, response) = retry_provider_request(
		|| client.create(&params, options.signal.clone(), options.timeout_ms),
		ProviderRetryOptions {
			max_retries: options.max_retries,
			max_retry_delay_ms: options.max_retry_delay_ms,
			signal: options.signal.clone(),
			no_retry_statuses: None,
		},
	)
	.await?;
	if let Some(on_response) = &options.on_response {
		on_response(
			ProviderResponse {
				status,
				headers: headers_to_record(&headers),
			},
			model,
		)
		.await?;
	}

	output.response_id = property(response.as_ref(), "id")?.and_then(|id| id.as_str().map(str::to_string));
	if let Some(usage) = property(response.as_ref(), "usage")?.filter(truthy) {
		output.usage = Some(parse_usage(usage, model));
	}

	let choices = property(response.as_ref(), "choices")?;
	let Some(choice) = property(choices, "0")?.filter(truthy) else {
		return Ok(());
	};
	let message = property(Some(choice), "message")?;
	if let Some(Value::String(content)) = property(message, "content")?
		&& !content.is_empty()
	{
		output.output.push(UserContent::Text(TextContent {
			text: content.clone(),
			..TextContent::default()
		}));
	}

	let images = match property(message, "images")? {
		None | Some(Value::Null) => &[][..],
		Some(Value::Array(images)) => images.as_slice(),
		Some(Value::String(_)) => &[][..],
		Some(other) => return Err(PiError::type_error(not_iterable(other))),
	};
	for image in images {
		let image_url = property(Some(image), "image_url")?;
		let image_url = match image_url {
			Some(Value::String(_)) => image_url,
			Some(Value::Object(image_url)) => image_url.get("url"),
			_ => None,
		};
		let image_url = match image_url {
			None | Some(Value::Null) => continue,
			Some(Value::String(image_url)) => image_url,
			Some(_) => return Err(PiError::type_error("imageUrl?.startsWith is not a function")),
		};
		if !image_url.starts_with("data:") {
			continue;
		}
		let Some(matches) = data_url_regex().captures(image_url) else {
			continue;
		};
		output.output.push(UserContent::Image(ImageContent {
			mime_type: matches[1].to_string(),
			data: matches[2].to_string(),
			..ImageContent::default()
		}));
	}
	Ok(())
}

/// `value[key]` for a JSON value: a TypeError on `undefined` and `null`, like V8.
fn property<'a>(value: Option<&'a Value>, key: &str) -> PiResult<Option<&'a Value>> {
	match value {
		None => Err(PiError::type_error(format!(
			"Cannot read properties of undefined (reading '{key}')"
		))),
		Some(Value::Null) => Err(PiError::type_error(format!(
			"Cannot read properties of null (reading '{key}')"
		))),
		Some(Value::Object(object)) => Ok(object.get(key)),
		Some(Value::Array(array)) => Ok(key.parse::<usize>().ok().and_then(|index| array.get(index))),
		Some(_) => Ok(None),
	}
}

fn truthy(value: &&Value) -> bool {
	match value {
		Value::Null => false,
		Value::Bool(value) => *value,
		Value::Number(number) => number.as_f64().is_some_and(|number| number != 0.0),
		Value::String(text) => !text.is_empty(),
		Value::Array(_) | Value::Object(_) => true,
	}
}

/// V8's `for...of` message for a non-iterable value.
fn not_iterable(value: &Value) -> String {
	let description = match value {
		Value::Number(number) => format!(
			"number {}",
			pi_host::js::number_to_string(number.as_f64().unwrap_or(0.0))
		),
		Value::Bool(value) => format!("boolean {value}"),
		_ => "object".to_string(),
	};
	format!("{description} is not iterable (cannot read property Symbol(Symbol.iterator))")
}

fn timed_out_regex() -> &'static Regex {
	static REGEX: std::sync::LazyLock<Regex> =
		std::sync::LazyLock::new(|| Regex::new("(?i)timed? ?out").expect("the timeout pattern is valid"));
	&REGEX
}

/// `/^data:([^;]+);base64,(.+)$/`: `.` excludes the JS line terminators.
fn data_url_regex() -> &'static Regex {
	static REGEX: std::sync::LazyLock<Regex> = std::sync::LazyLock::new(|| {
		Regex::new("^data:([^;]+);base64,([^\n\r\u{2028}\u{2029}]+)$").expect("the data URL pattern is valid")
	});
	&REGEX
}

fn create_client(model: &ImageModel, api_key: &str, options: &ImagesOptions) -> PiResult<OpenAiChatClient> {
	let identity_headers: ProviderHeaders = get_identity_headers(
		ClientIdentityTarget {
			provider: &model.provider,
			base_url: &model.base_url,
		},
		options.identity.as_ref(),
	)?
	.into_iter()
	.map(|(name, value)| (name, Some(value)))
	.collect();
	let model_headers: Option<ProviderHeaders> = model.headers.as_ref().map(|headers| {
		headers
			.iter()
			.map(|(name, value)| (name.clone(), Some(value.clone())))
			.collect()
	});
	let default_headers = provider_headers_to_record(&[
		Some(&identity_headers),
		model_headers.as_ref(),
		options.headers.as_ref(),
	]);
	OpenAiChatClient::new(api_key, &model.base_url, default_headers, options.fetch.clone())
}

fn build_params(model: &ImageModel, context: &ImagesContext) -> Value {
	let content: Vec<Value> = context
		.input
		.iter()
		.map(|item| match item {
			UserContent::Text(item) => json!({ "type": "text", "text": sanitize_surrogates(&item.text) }),
			UserContent::Image(item) => json!({
				"type": "image_url",
				"image_url": { "url": format!("data:{};base64,{}", item.mime_type, item.data) },
			}),
		})
		.collect();
	let modalities = if model.output.contains(&ModelInputModality::Text) {
		json!(["image", "text"])
	} else {
		json!(["image"])
	};
	json!({
		"model": model.id,
		"messages": [{ "role": "user", "content": content }],
		"stream": false,
		"modalities": modalities,
	})
}

fn parse_usage(raw_usage: &Value, model: &ImageModel) -> Usage {
	let count = |value: Option<&Value>| value.and_then(Value::as_f64).unwrap_or(0.0);
	let details = raw_usage.get("prompt_tokens_details");
	let prompt_tokens = count(raw_usage.get("prompt_tokens"));
	let reported_cached_tokens = count(details.and_then(|details| details.get("cached_tokens")));
	let cache_write_tokens = count(details.and_then(|details| details.get("cache_write_tokens")));
	let cache_read_tokens = if cache_write_tokens > 0.0 {
		pi_host::js::math_max(0.0, reported_cached_tokens - cache_write_tokens)
	} else {
		reported_cached_tokens
	};
	let input = pi_host::js::math_max(0.0, prompt_tokens - cache_read_tokens - cache_write_tokens);
	let output = count(raw_usage.get("completion_tokens"));
	let mut cost = UsageCost {
		input: (model.cost.input / 1000000.0) * input,
		output: (model.cost.output / 1000000.0) * output,
		cache_read: (model.cost.cache_read / 1000000.0) * cache_read_tokens,
		cache_write: (model.cost.cache_write / 1000000.0) * cache_write_tokens,
		total: 0.0,
		total_from_provider: Some(false),
		..UsageCost::default()
	};
	cost.total = cost.input + cost.output + cost.cache_read + cost.cache_write;
	Usage {
		input,
		output,
		cache_read: cache_read_tokens,
		cache_write: cache_write_tokens,
		total_tokens: input + output + cache_read_tokens + cache_write_tokens,
		cost,
		..Usage::default()
	}
}

const SDK_VERSION: &str = "7.19.0";
const DEFAULT_TIMEOUT_MS: f64 = 600_000.0;

/// The `openai` SDK client as one `chat.completions.create(params, { signal, timeout, maxRetries: 0 })` call sees it:
/// `buildURL`, `buildHeaders` and `validateHeaders`, `fetchWithTimeout` plus the body timeout, `defaultParseResponse`,
/// and `APIError.generate`. Errors carry `data.headers` (`null` for connection errors) so `retry_provider_request`
/// reads them as provider errors.
struct OpenAiChatClient {
	api_key: String,
	base_url: String,
	default_headers: Vec<(String, Option<String>)>,
	organization: Option<String>,
	project: Option<String>,
	fetch: Option<pi_host::FetchFunction>,
}

type ChatResponse = (u16, IndexMap<String, String>, Option<Value>);

impl OpenAiChatClient {
	/// `new OpenAI({ apiKey, baseURL, dangerouslyAllowBrowser: true, fetch, defaultHeaders })`.
	fn new(
		api_key: &str,
		base_url: &str,
		default_headers: Option<IndexMap<String, String>>,
		fetch: Option<pi_host::FetchFunction>,
	) -> PiResult<Self> {
		let mut default_headers: Vec<(String, Option<String>)> = default_headers
			.into_iter()
			.flatten()
			.map(|(name, value)| (name, Some(value)))
			.collect();
		if let Some(custom_headers) = read_env("OPENAI_CUSTOM_HEADERS") {
			let parsed: Vec<(String, Option<String>)> = custom_headers
				.split('\n')
				.filter_map(|line| {
					let colon = line.find(':')?;
					Some((
						js_trim(&line[..colon]).to_string(),
						Some(js_trim(&line[colon + 1..]).to_string()),
					))
				})
				.collect();
			let (values, nulls) = build_headers(&[parsed, default_headers])?;
			default_headers = values.into_iter().map(|(name, value)| (name, Some(value))).collect();
			default_headers.extend(nulls.into_iter().map(|name| (name, None)));
		}
		// `baseURL` is always given, so `OPENAI_BASE_URL` is never read, and an empty one falls back to the default.
		let base_url = if base_url.is_empty() {
			"https://api.openai.com/v1".to_string()
		} else {
			base_url.to_string()
		};
		Ok(Self {
			api_key: api_key.to_string(),
			base_url,
			default_headers,
			organization: read_env("OPENAI_ORG_ID"),
			project: read_env("OPENAI_PROJECT_ID"),
			fetch,
		})
	}

	/// `POST /chat/completions` with `maxRetries: 0`, resolving to the status, the response headers and the parsed
	/// body (`None` for `undefined`).
	fn create(
		&self,
		params: &Value,
		signal: Option<AbortSignal>,
		timeout_ms: Option<f64>,
	) -> LocalBoxFuture<'static, PiResult<ChatResponse>> {
		let request = self.build_request(params, timeout_ms);
		// `create` posts with `stream: body.stream ?? false`; a truthy value makes the SDK parse the response as SSE.
		let stream = params.get("stream").is_some_and(|stream| truthy(&stream));
		let fetch = self.fetch.clone();
		Box::pin(async move {
			let (mut request, timeout) = request?;
			let caller_aborted = || signal.as_ref().is_some_and(AbortSignal::aborted);
			if caller_aborted() {
				return Err(user_abort_error());
			}
			let start_ms = pi_host::now_ms();
			// `createRequestController`: the request signal follows the caller's signal and the SDK's own aborts.
			let controller = AbortController::new();
			request.signal = Some(match &signal {
				Some(signal) => AbortSignal::any(&[controller.signal(), signal.clone()]),
				None => controller.signal(),
			});
			// `fetchWithTimeout` aborts the request when its timer fires before the headers arrive, then clears it.
			let abort_on_timeout = async {
				let _ = pi_host::sleep(set_timeout_delay(timeout), None).await;
				controller.abort();
				futures::future::pending::<Infallible>().await
			};
			let response = match select(pin!(pi_host::fetch(request, fetch.as_ref())), pin!(abort_on_timeout)).await {
				Either::Left((response, _)) => response,
				Either::Right((never, _)) => match never {},
			};
			let response = match response {
				Ok(response) => response,
				Err(error) => {
					if caller_aborted() {
						return Err(user_abort_error());
					}
					// The SDK's `/timed? ?out/i` test on `String(error) + String(error.cause)`.
					let cause = error
						.data
						.as_ref()
						.and_then(|data| data.get("cause"))
						.and_then(Value::as_str);
					let described = format!("{}: {}{}", error.name, error.message, cause.unwrap_or(""));
					if error.is_abort() || timed_out_regex().is_match(&described) {
						return Err(connection_error("Request timed out."));
					}
					return Err(connection_error("Connection error."));
				}
			};
			let (status, headers) = (response.status, response.headers.clone());
			if !response.ok() {
				let text = match response.text().await {
					Ok(text) => text,
					Err(error) => error.message,
				};
				return Err(status_error(status, &headers, &text));
			}
			// `Stream.fromSSEResponse` reads nothing up front, and the `Stream` has no `id`, `usage` or `choices`.
			if stream {
				return Ok((status, headers, Some(json!({}))));
			}
			// `parseResponseWithTimeout` parses these directly, without its abort check and body timer.
			if status == 204 || response.header("content-length") == Some("0") {
				return Ok((status, headers, parse_response(response).await?));
			}
			if caller_aborted() {
				return Err(user_abort_error());
			}
			let remaining = (start_ms + timeout - pi_host::now_ms()).max(0.0);
			let interrupt = async {
				let caller_abort = async {
					match &signal {
						Some(signal) => signal.aborted_future().await,
						None => futures::future::pending().await,
					}
				};
				match select(
					pin!(pi_host::sleep(set_timeout_delay(remaining), None)),
					pin!(caller_abort),
				)
				.await
				{
					Either::Left(_) => {
						controller.abort();
						connection_error("Request timed out.")
					}
					Either::Right(_) => user_abort_error(),
				}
			};
			match select(pin!(parse_response(response)), pin!(interrupt)).await {
				Either::Left((Ok(body), _)) => Ok((status, headers, body)),
				Either::Left((Err(_), _)) if caller_aborted() => Err(user_abort_error()),
				Either::Left((Err(error), _)) | Either::Right((error, _)) => Err(error),
			}
		})
	}

	/// `buildRequest`: the URL, the headers and the body, plus the timeout in milliseconds.
	fn build_request(&self, params: &Value, timeout_ms: Option<f64>) -> PiResult<(HttpRequest, f64)> {
		if params.is_null() {
			return Err(PiError::type_error("Cannot read properties of null (reading 'stream')"));
		}
		let url = self.build_url("/chat/completions")?;
		if let Some(timeout) = timeout_ms {
			if timeout.fract() != 0.0 || !timeout.is_finite() {
				return Err(PiError::new("timeout must be an integer"));
			}
			if timeout < 0.0 {
				return Err(PiError::new("timeout must be a positive integer"));
			}
		}
		let timeout = timeout_ms.unwrap_or(DEFAULT_TIMEOUT_MS);
		let mut sdk_headers: Vec<(String, Option<String>)> = vec![
			("Accept".into(), Some("application/json".into())),
			("User-Agent".into(), Some(format!("OpenAI/JS {SDK_VERSION}"))),
			("X-Stainless-Retry-Count".into(), Some("0".into())),
		];
		// The SDK reads the timeout header from the caller's request options, so the 10-minute default sends none.
		if let Some(timeout) = timeout_ms.filter(|timeout| *timeout != 0.0) {
			sdk_headers.push((
				"X-Stainless-Timeout".into(),
				Some(pi_host::js::number_to_string((timeout / 1000.0).trunc())),
			));
		}
		sdk_headers.extend(
			platform_headers()
				.into_iter()
				.map(|(name, value)| (name.to_string(), Some(value))),
		);
		sdk_headers.push(("OpenAI-Organization".into(), self.organization.clone()));
		sdk_headers.push(("OpenAI-Project".into(), self.project.clone()));
		// `buildBody` sends no body and no content type for a falsy body.
		let has_body = truthy(&params);
		let body_headers = if has_body {
			vec![("content-type".into(), Some("application/json".into()))]
		} else {
			Vec::new()
		};
		let (values, nulls) = build_headers(&[
			sdk_headers,
			vec![("Authorization".into(), Some(format!("Bearer {}", self.api_key)))],
			self.default_headers.clone(),
			body_headers,
		])?;
		let has = |name: &str| values.get(name).is_some_and(|value| !value.is_empty()) || nulls.contains(name);
		if !has("authorization") && !has("api-key") {
			return Err(PiError::new(
				"Could not resolve authentication method. Expected either apiKey or adminAPIKey to be set. Or for one of the \"Authorization\" or \"api-key\" headers to be explicitly omitted",
			));
		}
		let mut request = HttpRequest::new("POST", url);
		request.headers = values;
		request.body = has_body.then(|| json_stringify(params).into());
		Ok((request, timeout))
	}

	/// `buildURL(path)`: the base URL joined with the path, with an existing query re-serialized by `qs`.
	fn build_url(&self, path: &str) -> PiResult<String> {
		let joined = if self.base_url.ends_with('/') && path.starts_with('/') {
			format!("{}{}", self.base_url, &path[1..])
		} else {
			format!("{}{path}", self.base_url)
		};
		let mut url = url::Url::parse(&joined).map_err(|_| PiError::type_error("Invalid URL"))?;
		let mut query: IndexMap<String, String> = url.query_pairs().into_owned().collect();
		// `Object.fromEntries(url.searchParams)` makes an object, which lists array-index keys first, ascending.
		let array_index = |key: &str| {
			key.parse::<u32>()
				.ok()
				.filter(|index| *index != u32::MAX && index.to_string() == key)
		};
		query.sort_by(|a, _, b, _| match (array_index(a), array_index(b)) {
			(Some(a), Some(b)) => a.cmp(&b),
			(Some(_), None) => std::cmp::Ordering::Less,
			(None, Some(_)) => std::cmp::Ordering::Greater,
			(None, None) => std::cmp::Ordering::Equal,
		});
		if !query.is_empty() {
			let search: Vec<String> = query
				.iter()
				.map(|(key, value)| format!("{}={}", qs_encode(key), qs_encode(value)))
				.collect();
			url.set_query(Some(&search.join("&")));
		}
		Ok(url.to_string())
	}
}

/// The delay `setTimeout` waits: one beyond `2^31 - 1` is clamped to 1ms.
fn set_timeout_delay(ms: f64) -> f64 {
	if ms > 2_147_483_647.0 { 1.0 } else { ms }
}

/// `defaultParseResponse` for a non-streaming JSON call: `None` stands for `undefined`.
async fn parse_response(response: HttpResponse) -> PiResult<Option<Value>> {
	if response.status == 204 {
		return Ok(Some(Value::Null));
	}
	let media_type = response
		.header("content-type")
		.and_then(|content_type| content_type.split(';').next())
		.map(|media_type| js_trim(media_type).to_lowercase());
	let is_json = media_type
		.as_deref()
		.is_some_and(|media_type| media_type.contains("application/json") || media_type.ends_with("+json"));
	if !is_json {
		return Ok(Some(Value::String(response.text().await?)));
	}
	if response.header("content-length") == Some("0") {
		return Ok(None);
	}
	let text = response.text().await?;
	if text.is_empty() {
		return Ok(None);
	}
	json_parse(&text).map(Some)
}

/// `makeStatusError(status, safeJSON(text), message, headers)` through `APIError.generate`.
fn status_error(status: u16, headers: &IndexMap<String, String>, text: &str) -> PiError {
	let error_json = json_parse(text).ok();
	let message = if error_json.as_ref().is_some_and(|value| truthy(&value)) {
		None
	} else {
		Some(text)
	};
	let normalized = match error_json {
		Some(Value::Object(object)) if object.get("error").is_none_or(Value::is_null) => {
			json!({ "error": Value::Object(object) })
		}
		Some(Value::Array(array)) => json!({ "error": Value::Array(array) }),
		Some(other) => other,
		None => Value::Null,
	};
	let error = normalized.as_object().and_then(|object| object.get("error")).cloned();
	let error_message = error.as_ref().and_then(|error| error.get("message")).filter(truthy);
	let msg = match (error_message, &error) {
		(Some(Value::String(message)), _) => Some(message.clone()),
		(Some(message), _) => Some(json_stringify(message)),
		(None, Some(error)) if truthy(&error) => Some(json_stringify(error)),
		_ => message.map(str::to_string),
	};
	let message = match msg.filter(|msg| !msg.is_empty()) {
		Some(msg) => format!("{status} {msg}"),
		None => format!("{status} status code (no body)"),
	};
	let mut data = Map::new();
	data.insert("headers".into(), json!(headers_to_record(headers)));
	if let Some(error) = error {
		for field in ["code", "param", "type"] {
			if let Some(value) = error.get(field) {
				data.insert(field.into(), value.clone());
			}
		}
		data.insert("error".into(), error);
	}
	PiError::new(message).with_status(status).with_data(Value::Object(data))
}

fn user_abort_error() -> PiError {
	PiError::new("Request was aborted.").with_data(json!({ "headers": null }))
}

fn connection_error(message: &str) -> PiError {
	PiError::new(message).with_data(json!({ "headers": null }))
}

/// `buildHeaders` over plain-object sources: each source overwrites the names it lists, `null` removes a name and
/// records it as explicitly omitted. Returns the `Headers` (lowercase names in iteration order, which is sorted) and
/// the omitted names.
fn build_headers(sources: &[Vec<(String, Option<String>)>]) -> PiResult<(IndexMap<String, String>, HashSet<String>)> {
	let mut values: IndexMap<String, String> = IndexMap::new();
	let mut nulls = HashSet::new();
	for source in sources {
		for (name, value) in source {
			if name.is_empty() || !name.bytes().all(is_http_token_byte) {
				return Err(PiError::type_error(format!(
					"Header name must be a valid HTTP token [\"{name}\"]"
				)));
			}
			let name = name.to_ascii_lowercase();
			match value {
				Some(value) => {
					values.insert(name.clone(), value.trim_matches([' ', '\t', '\n', '\r']).to_string());
					nulls.remove(&name);
				}
				None => {
					values.shift_remove(&name);
					nulls.insert(name);
				}
			}
		}
	}
	values.sort_keys();
	Ok((values, nulls))
}

/// `/^[!#$%&'*+\-.^_`|~0-9A-Za-z]+$/`.
fn is_http_token_byte(byte: u8) -> bool {
	byte.is_ascii_alphanumeric() || b"!#$%&'*+-.^_|~`".contains(&byte)
}

/// `qs`'s RFC 3986 encoder: unreserved characters stay, every other UTF-8 byte is percent-encoded.
fn qs_encode(text: &str) -> String {
	let mut encoded = String::new();
	for byte in text.bytes() {
		if byte.is_ascii_alphanumeric() || b"-_.~".contains(&byte) {
			encoded.push(byte as char);
		} else {
			encoded.push_str(&format!("%{byte:02X}"));
		}
	}
	encoded
}

/// `readEnv(name)`: the trimmed value, or `None` when unset or blank.
fn read_env(name: &str) -> Option<String> {
	pi_host::env(name)
		.map(|value| js_trim(&value).to_string())
		.filter(|value| !value.is_empty())
}

/// `getPlatformHeaders()`.
fn platform_headers() -> [(&'static str, String); 6] {
	let (os, arch, runtime) = if cfg!(target_arch = "wasm32") {
		("Unknown".to_string(), "unknown".to_string(), "unknown")
	} else {
		let os = match std::env::consts::OS {
			"linux" => "Linux".to_string(),
			"macos" => "MacOS".to_string(),
			"windows" => "Windows".to_string(),
			"freebsd" => "FreeBSD".to_string(),
			"openbsd" => "OpenBSD".to_string(),
			"android" => "Android".to_string(),
			"ios" => "iOS".to_string(),
			other => format!("Other:{other}"),
		};
		let arch = match std::env::consts::ARCH {
			"x86_64" => "x64".to_string(),
			"aarch64" => "arm64".to_string(),
			"arm" => "arm".to_string(),
			"x86" => "other:ia32".to_string(),
			other => format!("other:{other}"),
		};
		(os, arch, "node")
	};
	[
		("X-Stainless-Lang", "js".to_string()),
		("X-Stainless-Package-Version", SDK_VERSION.to_string()),
		("X-Stainless-OS", os),
		("X-Stainless-Arch", arch),
		("X-Stainless-Runtime", runtime.to_string()),
		("X-Stainless-Runtime-Version", "unknown".to_string()),
	]
}
