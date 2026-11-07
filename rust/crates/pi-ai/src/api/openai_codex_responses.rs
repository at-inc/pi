//! Port of `packages/ai/src/api/openai-codex-responses.ts`.
//! PORT-STATUS: ported
//!
//! - `OpenAICodexResponsesOptions` reaches `ProviderStreams::stream` as `ProviderStreamOptions.extra` keys:
//!   `reasoningEffort` (a string; another type reads as absent), and `reasoningSummary`, `serviceTier`,
//!   `textVerbosity` and `toolChoice` (any JSON, used as the TS uses them). The image-generation hooks are functions,
//!   so only direct callers of `stream` can set them.
//! - The request body is a JSON value in TS key order; `undefined` fields are left out, as `JSON.stringify` drops
//!   them. A payload hook that returns a non-object spreads as no keys into the WebSocket `response.create` frame.
//! - Error classes are `PiError` names (`CodexApiError`, `CodexProtocolError`, `ProviderStreamEventCallbackError`,
//!   `WebSocketCloseError`), and `instanceof` checks compare names. `CodexApiError` carries `status` in
//!   `PiError.status` and `code`, `payload` and `headers` in `data`. `RetryDelayExceededError` is tracked by the retry
//!   loop instead of by class.
//! - `Headers` objects are lowercase name maps: `new Headers(record)` appends, `set` replaces, values are trimmed of
//!   HTTP whitespace, and names are sent sorted. Invalid header names or values are not rejected.
//! - `new URL` of the WebSocket URL uses the `url` crate's serialization.
//! - The SSE body is zstd level 3 with the `native` feature (the `zstd` crate, whose bytes may differ from Node's
//!   `zstdCompressSync` but decode to the same JSON); without it the body is sent uncompressed, as the TS does where
//!   `node:zlib` is missing.
//! - WebSockets come from the host capability (`pi_host::websocket`). Bun's proxy-aware `WebSocket` subclass has no
//!   counterpart, so `env` is not read, and the native host has no HTTP proxy support. A WebSocket `error` event
//!   carries only a message; a binary frame is decoded as UTF-8 like `TextDecoder`. Message events are handled
//!   synchronously, without the microtask the TS `async` handler waits for.
//! - JSON events that are `null` fail with the `TypeError` V8 throws when reading `type` from them.
//! - `registerSessionResourceCleanup(closeOpenAICodexWebSocketSessions)` runs on the first `stream` call of a thread
//!   instead of at import, and the session caches are per thread.
//! - The async body runs on the host executor from the first step; the output timestamp is taken in `stream()`.
//! - The response always has a body, so `No response body` is never thrown.

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet, VecDeque};
use std::rc::Rc;
use std::sync::LazyLock;

use bytes::Bytes;
use futures::StreamExt;
use futures::channel::oneshot;
use futures::future::{Either, select};
use indexmap::IndexMap;
use pi_host::abort::ListenerId;
use pi_host::js::{date_parse, json_stringify, math_max, number_to_string, string_to_number};
use pi_host::websocket::{OPEN, WebSocket, WebSocketData, WebSocketEvent, WebSocketListenerId};
use pi_host::{AbortController, AbortSignal, HttpRequest, HttpResponse, LocalBoxStream, PiError, PiResult};
use regex::Regex;
use serde_json::{Map, Value, json};

use crate::api::constrained_sampling::create_grammar_tool_input_properties;
use crate::api::openai_codex_images::atob;
use crate::api::openai_prompt_cache::clamp_openai_prompt_cache_key;
use crate::api::openai_responses_shared::{
	ConvertResponsesMessagesOptions, ConvertResponsesToolsOptions, OnImageGenerationCall,
	OnImageGenerationPartialImage, OpenAIResponsesStreamOptions, convert_responses_messages, convert_responses_tools,
	get_responses_tool_support, js_string, process_responses_stream, truthy,
};
use crate::api::simple_options::build_base_options;
use crate::models::clamp_thinking_level;
use crate::session_resources::register_session_resource_cleanup;
use crate::types::{
	AssistantMessage, AssistantMessageEvent, CacheRetention, Context, JsonObject, Message, Model, ModelThinkingLevel,
	OnProviderStreamEvent, ProviderHeaders, ProviderResponse, ProviderStreamOptions, ProviderStreams,
	SharedAssistantMessage, SimpleStreamOptions, StopReason, StreamOptions, ToolChoice, TranscriptContext, Transport,
	Usage, UsageCost,
};
use crate::utils::abort_signals::combine_abort_signals;
use crate::utils::diagnostics::{
	append_assistant_message_diagnostic, create_assistant_message_diagnostic, create_provider_error_diagnostic,
	format_thrown_value,
};
use crate::utils::error_body::{format_provider_error, normalize_provider_error};
use crate::utils::event_stream::AssistantMessageEventStream;
use crate::utils::headers::headers_to_record;
use crate::utils::identity::{ClientIdentityTarget, get_identity_headers};
use crate::utils::json_parse::json_parse;
use crate::utils::text::{get_system_message_text, js_trim};
use crate::utils::transcript::{
	get_declared_tools, get_initial_system_message, normalize_context, resolve_transcript, resolve_transcript_tools,
};
use crate::utils::uuid::uuidv7;

// ============================================================================
// Configuration
// ============================================================================

const DEFAULT_CODEX_BASE_URL: &str = "https://chatgpt.com/backend-api";
const JWT_CLAIM_PATH: &str = "https://api.openai.com/auth";
const DEFAULT_MAX_RETRIES: f64 = 0.0;
const BASE_DELAY_MS: f64 = 1000.0;
const DEFAULT_MAX_RETRY_DELAY_MS: f64 = 60_000.0;
const DEFAULT_WEBSOCKET_CONNECT_TIMEOUT_MS: f64 = 15_000.0;
// The Codex backend accepts zstd-compressed request bodies on the SSE responses
// endpoint (the same endpoint the official Codex client compresses against).
#[cfg(feature = "native")]
const REQUEST_COMPRESSION_ZSTD_LEVEL: i32 = 3;
const CODEX_TOOL_CALL_PROVIDERS: [&str; 3] = ["openai", "openai-codex", "opencode"];
const WEBSOCKET_MESSAGE_TOO_BIG_CLOSE_CODE: u16 = 1009;
const WEBSOCKET_CONNECTION_LIMIT_REACHED_CODE: &str = "websocket_connection_limit_reached";
const PREVIOUS_RESPONSE_NOT_FOUND_CODE: &str = "previous_response_not_found";

const CODEX_RESPONSE_STATUSES: [&str; 6] = [
	"completed",
	"incomplete",
	"failed",
	"cancelled",
	"queued",
	"in_progress",
];

// ============================================================================
// Types
// ============================================================================

/// Codex Responses options.
#[derive(Clone, Default)]
pub struct OpenAICodexResponsesOptions {
	pub base: StreamOptions,
	/// `"none" | "minimal" | "low" | "medium" | "high" | "xhigh" | "max"`.
	pub reasoning_effort: Option<String>,
	/// `"auto" | "concise" | "detailed" | "off" | "on" | null`, sent as given.
	pub reasoning_summary: Option<Value>,
	/// `ResponseCreateParamsStreaming["service_tier"]`, sent as given.
	pub service_tier: Option<Value>,
	/// `"low" | "medium" | "high"`, sent as given.
	pub text_verbosity: Option<Value>,
	/// `"auto" | "none" | "required"`, sent as given.
	pub tool_choice: Option<Value>,
	/// Internal native-tool output hook used by the dedicated image transport.
	pub on_image_generation_call: Option<OnImageGenerationCall>,
	pub on_image_generation_partial_image: Option<OnImageGenerationPartialImage>,
}

impl std::ops::Deref for OpenAICodexResponsesOptions {
	type Target = StreamOptions;

	fn deref(&self) -> &Self::Target {
		&self.base
	}
}

impl std::ops::DerefMut for OpenAICodexResponsesOptions {
	fn deref_mut(&mut self) -> &mut Self::Target {
		&mut self.base
	}
}

impl OpenAICodexResponsesOptions {
	/// Reads the option keys of this API from `ProviderStreamOptions.extra`.
	pub fn from_provider_options(options: ProviderStreamOptions) -> Self {
		let ProviderStreamOptions { base, extra } = options;
		Self {
			base,
			reasoning_effort: extra.get("reasoningEffort").and_then(Value::as_str).map(str::to_string),
			reasoning_summary: extra.get("reasoningSummary").cloned(),
			service_tier: extra.get("serviceTier").cloned(),
			text_verbosity: extra.get("textVerbosity").cloned(),
			tool_choice: extra.get("toolChoice").cloned(),
			..Self::default()
		}
	}
}

fn assert_successful_output(output: &SharedAssistantMessage) -> PiResult<()> {
	let output = output.borrow();
	if output.stop_reason == StopReason::Pending {
		return Err(PiError::new("Codex stream ended without a stop reason"));
	}
	if output.stop_reason == StopReason::Error || output.stop_reason == StopReason::Aborted {
		return Err(PiError::new(
			output
				.error_message
				.clone()
				.filter(|message| !message.is_empty())
				.unwrap_or_else(|| "An unknown error occurred".into()),
		));
	}
	Ok(())
}

// ============================================================================
// Retry Helpers
// ============================================================================

fn is_terminal_rate_limit_error(error_text: &str) -> bool {
	let text = error_text.to_ascii_lowercase();
	[
		"gousagelimiterror",
		"freeusagelimiterror",
		"monthly usage limit reached",
		"available balance",
		"insufficient_quota",
		"out of budget",
		"quota exceeded",
		"billing",
	]
	.iter()
	.any(|pattern| text.contains(pattern))
}

// `/.../i` over an ASCII pattern: JS case folding never maps a non-ASCII character to an ASCII one, and `.` excludes
// the JS line terminators and matches one UTF-16 code unit, so never a character outside the BMP.
static RETRYABLE_ERROR_TEXT: LazyLock<Regex> = LazyLock::new(|| {
	Regex::new(
		"rate[^\n\r\u{2028}\u{2029}\u{10000}-\u{10FFFF}]?limit|overloaded|service[^\n\r\u{2028}\u{2029}\u{10000}-\u{10FFFF}]?unavailable|upstream[^\n\r\u{2028}\u{2029}\u{10000}-\u{10FFFF}]?connect|connection[^\n\r\u{2028}\u{2029}\u{10000}-\u{10FFFF}]?refused",
	)
	.expect("valid regex")
});

fn is_retryable_error(status: u16, error_text: &str) -> bool {
	if status == 429 && is_terminal_rate_limit_error(error_text) {
		return false;
	}
	if status == 429 || status == 500 || status == 502 || status == 503 || status == 504 {
		return true;
	}
	RETRYABLE_ERROR_TEXT.is_match(&error_text.to_ascii_lowercase())
}

fn get_retry_after_delay_ms(headers: &IndexMap<String, String>) -> Option<f64> {
	let headers = headers_to_record(headers);
	if let Some(retry_after_ms) = headers.get("retry-after-ms") {
		let millis = string_to_number(retry_after_ms);
		if millis.is_finite() {
			return Some(math_max(0.0, millis));
		}
	}

	let retry_after = headers.get("retry-after").filter(|value| !value.is_empty())?;

	let seconds = string_to_number(retry_after);
	if seconds.is_finite() {
		return Some(math_max(0.0, seconds * 1000.0));
	}

	let date = date_parse(retry_after);
	if !date.is_nan() {
		return Some(math_max(0.0, date - pi_host::now_ms()));
	}

	None
}

/// A failed SSE attempt: `RetryDelayExceededError` is never retried.
enum AttemptError {
	RetryDelayExceeded(PiError),
	Other(PiError),
}

fn validate_retry_delay_ms(delay_ms: f64, options: Option<&OpenAICodexResponsesOptions>) -> Result<f64, AttemptError> {
	let max_retry_delay_ms = options
		.and_then(|options| options.max_retry_delay_ms)
		.unwrap_or(DEFAULT_MAX_RETRY_DELAY_MS);
	if max_retry_delay_ms > 0.0 && delay_ms > max_retry_delay_ms {
		return Err(AttemptError::RetryDelayExceeded(PiError::new(format!(
			"Server requested {}s retry delay (max: {}s)",
			number_to_string((delay_ms / 1000.0).ceil()),
			number_to_string((max_retry_delay_ms / 1000.0).ceil())
		))));
	}
	Ok(delay_ms)
}

async fn sleep(ms: f64, signal: Option<&AbortSignal>) -> PiResult<()> {
	if signal.is_some_and(AbortSignal::aborted) {
		return Err(PiError::new("Request was aborted"));
	}
	pi_host::sleep(ms, signal)
		.await
		.map_err(|_| PiError::new("Request was aborted"))
}

fn normalize_timeout_ms(value: Option<f64>) -> PiResult<Option<f64>> {
	let Some(value) = value else {
		return Ok(None);
	};
	if !value.is_finite() || value < 0.0 {
		return Err(PiError::new(format!("Invalid timeoutMs: {}", number_to_string(value))));
	}
	Ok(Some(value.floor()))
}

// ============================================================================
// Request Compression
// ============================================================================

// Returns the zstd-compressed body bytes, or None when compression is
// unavailable. Callers fall back to sending the uncompressed JSON when this returns None.
#[cfg(feature = "native")]
fn compress_request_body_zstd(body_json: &str) -> Option<Vec<u8>> {
	zstd::bulk::compress(body_json.as_bytes(), REQUEST_COMPRESSION_ZSTD_LEVEL).ok()
}

#[cfg(not(feature = "native"))]
fn compress_request_body_zstd(_body_json: &str) -> Option<Vec<u8>> {
	None
}

// ============================================================================
// Headers
// ============================================================================

/// A `Headers` object: lowercase names, in insertion order until `record` sorts them.
#[derive(Clone, Default)]
struct CodexHeaders(IndexMap<String, String>);

impl CodexHeaders {
	/// `new Headers(record)`.
	fn from_record(record: &IndexMap<String, String>) -> Self {
		let mut headers = Self::default();
		for (name, value) in record {
			let value = trim_header_value(value);
			match headers.0.get_mut(&name.to_ascii_lowercase()) {
				Some(existing) => {
					existing.push_str(", ");
					existing.push_str(value);
				}
				None => {
					headers.0.insert(name.to_ascii_lowercase(), value.to_string());
				}
			}
		}
		headers
	}

	/// `headers.set(name, value)`.
	fn set(&mut self, name: &str, value: &str) {
		self.0
			.insert(name.to_ascii_lowercase(), trim_header_value(value).to_string());
	}

	/// `headers.delete(name)`.
	fn delete(&mut self, name: &str) {
		self.0.shift_remove(&name.to_ascii_lowercase());
	}

	/// `headersToRecord(headers)`, which is also the order `fetch` sends them in.
	fn record(&self) -> IndexMap<String, String> {
		headers_to_record(&self.0)
	}
}

fn trim_header_value(value: &str) -> &str {
	value.trim_matches([' ', '\t', '\r', '\n'])
}

// ============================================================================
// Main Stream Function
// ============================================================================

/// The `openai-codex-responses.ts` module as `ProviderStreams`.
pub struct OpenAICodexResponsesApi;

impl ProviderStreams for OpenAICodexResponsesApi {
	fn stream(
		&self,
		model: &Model,
		context: &TranscriptContext,
		options: Option<ProviderStreamOptions>,
	) -> PiResult<AssistantMessageEventStream> {
		Ok(stream(
			model,
			context,
			options.map(OpenAICodexResponsesOptions::from_provider_options),
		))
	}

	fn stream_simple(
		&self,
		model: &Model,
		context: &TranscriptContext,
		options: Option<SimpleStreamOptions>,
	) -> PiResult<AssistantMessageEventStream> {
		stream_simple(model, context, options)
	}
}

pub fn stream(
	model: &Model,
	context: &TranscriptContext,
	options: Option<OpenAICodexResponsesOptions>,
) -> AssistantMessageEventStream {
	ensure_session_resource_cleanup_registered();
	let stream = AssistantMessageEventStream::new();
	let normalized_context = resolve_transcript(
		context,
		model.openai_responses_compat().supports_mid_convo_system_messages,
	);

	let output: SharedAssistantMessage = Rc::new(RefCell::new(AssistantMessage {
		content: Vec::new(),
		api: "openai-codex-responses".into(),
		provider: model.provider.clone(),
		model: model.id.clone(),
		usage: Usage {
			input: 0.0,
			output: 0.0,
			cache_read: 0.0,
			cache_write: 0.0,
			total_tokens: 0.0,
			cost: UsageCost::default(),
			..Usage::default()
		},
		stop_reason: StopReason::Pending,
		timestamp: pi_host::now_ms(),
		..AssistantMessage::default()
	}));

	let (model, target) = (model.clone(), stream.clone());
	pi_host::spawn_local(async move {
		let options = options.as_ref();
		let Err(error) = run_stream(&model, &normalized_context, options, &output, &target).await else {
			return;
		};
		let stop_reason = if options
			.and_then(|options| options.signal.as_ref())
			.is_some_and(AbortSignal::aborted)
		{
			StopReason::Aborted
		} else {
			StopReason::Error
		};
		{
			let mut output = output.borrow_mut();
			output.stop_reason = stop_reason;
			output.error_message = Some(format_provider_error(&normalize_provider_error(&error), None));
			if stop_reason == StopReason::Error {
				append_assistant_message_diagnostic(&mut output, create_provider_error_diagnostic(&error));
			}
		}
		target.push(AssistantMessageEvent::Error {
			reason: stop_reason,
			error: output.clone(),
		});
		target.end(None);
	});

	stream
}

/// What one Codex request needs on every transport.
struct CodexRequest<'a> {
	model: &'a Model,
	output: &'a SharedAssistantMessage,
	stream: &'a AssistantMessageEventStream,
	grammar_tool_input_properties: &'a IndexMap<String, String>,
	options: Option<&'a OpenAICodexResponsesOptions>,
	http_timeout_ms: Option<f64>,
	websocket_connect_timeout_ms: Option<f64>,
	cache_session_id: Option<&'a str>,
	account_id: &'a str,
}

impl CodexRequest<'_> {
	fn signal(&self) -> Option<&AbortSignal> {
		self.options.and_then(|options| options.signal.as_ref())
	}

	fn aborted(&self) -> bool {
		self.signal().is_some_and(AbortSignal::aborted)
	}

	fn push_done(&self) {
		let stop_reason = self.output.borrow().stop_reason;
		self.stream.push(AssistantMessageEvent::Done {
			reason: stop_reason,
			message: self.output.clone(),
		});
		self.stream.end(None);
	}
}

async fn run_stream(
	model: &Model,
	normalized_context: &TranscriptContext,
	options: Option<&OpenAICodexResponsesOptions>,
	output: &SharedAssistantMessage,
	stream: &AssistantMessageEventStream,
) -> PiResult<()> {
	let Some(api_key) = options
		.and_then(|options| options.api_key.as_deref())
		.filter(|api_key| !api_key.is_empty())
	else {
		return Err(PiError::new(format!("No API key for provider: {}", model.provider)));
	};

	let account_id = extract_account_id(api_key)?;
	let grammar_tool_input_properties = create_grammar_tool_input_properties(
		Some(&get_declared_tools(&normalized_context.messages)),
		model
			.openai_responses_compat()
			.supports_openai_grammar_tools
			.unwrap_or(false),
	)?;
	let cache_session_id = if options.and_then(|options| options.cache_retention) == Some(CacheRetention::None) {
		None
	} else {
		options.and_then(|options| options.session_id.as_deref())
	};
	let codex_session_id = clamp_openai_prompt_cache_key(cache_session_id);
	let mut body = build_request_body(
		model,
		normalized_context,
		options,
		codex_session_id.as_deref(),
		&grammar_tool_input_properties,
	)?;
	if let Some(on_payload) = options.and_then(|options| options.on_payload.as_ref())
		&& let Some(next_body) = on_payload(body.clone(), model).await?
	{
		body = next_body;
	}
	let websocket_request_id = match codex_session_id.as_deref().filter(|id| !id.is_empty()) {
		Some(id) => id.to_string(),
		None => uuidv7(None)?,
	};
	// Codex expects an originator on every request; it is not attribution.
	let mut identity_headers = IndexMap::from([("originator".to_string(), "pi".to_string())]);
	identity_headers.extend(get_identity_headers(
		ClientIdentityTarget {
			provider: &model.provider,
			base_url: &model.base_url,
		},
		options.and_then(|options| options.identity.as_ref()),
	)?);
	let model_headers = model.headers.as_ref();
	let request_headers = options.and_then(|options| options.headers.as_ref());
	let mut sse_headers = build_sse_headers(
		&identity_headers,
		model_headers,
		request_headers,
		&account_id,
		api_key,
		codex_session_id.as_deref(),
	);
	let websocket_headers = build_websocket_headers(
		&identity_headers,
		model_headers,
		request_headers,
		&account_id,
		api_key,
		&websocket_request_id,
	);
	let body_json = json_stringify(&body);
	let request = CodexRequest {
		model,
		output,
		stream,
		grammar_tool_input_properties: &grammar_tool_input_properties,
		options,
		http_timeout_ms: normalize_timeout_ms(options.and_then(|options| options.timeout_ms))?,
		websocket_connect_timeout_ms: normalize_timeout_ms(
			options.and_then(|options| options.websocket_connect_timeout_ms),
		)?,
		cache_session_id,
		account_id: &account_id,
	};
	let transport = options.and_then(|options| options.transport).unwrap_or(Transport::Auto);
	let start_emitted = Rc::new(Cell::new(false));
	let websocket_disabled_for_session =
		transport != Transport::Sse && is_websocket_sse_fallback_active(cache_session_id);
	if websocket_disabled_for_session {
		record_websocket_sse_fallback(cache_session_id);
	}

	if transport != Transport::Sse && !websocket_disabled_for_session {
		let websocket_started = Rc::new(Cell::new(false));
		let mut retried_websocket_connection_limit = false;
		let mut retried_missing_websocket_continuation = false;
		let on_start: Rc<dyn Fn()> = {
			let (websocket_started, start_emitted) = (websocket_started.clone(), start_emitted.clone());
			let (stream, output) = (stream.clone(), output.clone());
			Rc::new(move || {
				websocket_started.set(true);
				if !start_emitted.get() {
					start_emitted.set(true);
					stream.push(AssistantMessageEvent::Start {
						partial: output.clone(),
					});
				}
			})
		};
		loop {
			websocket_started.set(false);
			let result = async {
				let url = resolve_codex_websocket_url(&model.base_url)?;
				process_websocket_stream(&url, &body, &websocket_headers, on_start.clone(), &request).await?;
				if request.aborted() {
					return Err(PiError::new("Request was aborted"));
				}
				assert_successful_output(output)
			}
			.await;
			let error = match result {
				Ok(()) => {
					request.push_done();
					return Ok(());
				}
				Err(error) => error,
			};
			let aborted = request.aborted();
			let connection_limit_before_start =
				!websocket_started.get() && is_websocket_connection_limit_reached_error(&error);
			let previous_response_not_found = is_previous_response_not_found_error(&error);
			if !aborted && previous_response_not_found && !retried_missing_websocket_continuation {
				retried_missing_websocket_continuation = true;
				continue;
			}
			if !aborted && connection_limit_before_start && !retried_websocket_connection_limit {
				retried_websocket_connection_limit = true;
				continue;
			}
			if aborted || (is_codex_non_transport_error(&error) && !connection_limit_before_start) {
				return Err(error);
			}
			let started = websocket_started.get();
			let mut details = JsonObject::new();
			details.insert("configuredTransport".into(), json!(transport_name(transport)));
			if !started {
				details.insert("fallbackTransport".into(), json!("sse"));
			}
			details.insert("eventsEmitted".into(), json!(started));
			details.insert(
				"phase".into(),
				json!(if started {
					"after_message_stream_start"
				} else {
					"before_message_stream_start"
				}),
			);
			details.insert("requestBytes".into(), json!(body_json.len()));
			append_assistant_message_diagnostic(
				&mut output.borrow_mut(),
				create_assistant_message_diagnostic("provider_transport_failure", &error, Some(details)),
			);
			record_websocket_failure(cache_session_id, &error);
			if started {
				return Err(error);
			}
			record_websocket_sse_fallback(cache_session_id);
			break;
		}
	}

	// Compress the request body once for the SSE path. The Codex backend
	// decodes Content-Encoding: zstd; the WebSocket transport above sends the
	// uncompressed JSON frame, matching the official Codex client.
	let compressed_body = compress_request_body_zstd(&body_json);
	if compressed_body.is_some() {
		sse_headers.set("content-encoding", "zstd");
	}
	let sse_body = Bytes::from(compressed_body.unwrap_or_else(|| body_json.clone().into_bytes()));

	// Fetch with retry logic for rate limits and transient errors
	let response = fetch_with_retries(&sse_headers, sse_body, &request).await?;

	if !start_emitted.get() {
		start_emitted.set(true);
		stream.push(AssistantMessageEvent::Start {
			partial: output.clone(),
		});
	}
	process_stream(response, &request).await?;

	if request.aborted() {
		return Err(PiError::new("Request was aborted"));
	}

	assert_successful_output(output)?;
	request.push_done();
	Ok(())
}

fn transport_name(transport: Transport) -> &'static str {
	match transport {
		Transport::Sse => "sse",
		Transport::Websocket => "websocket",
		Transport::WebsocketCached => "websocket-cached",
		Transport::Auto => "auto",
	}
}

/// The SSE `fetch` loop: the `ok` response, retrying rate limits, transient statuses and network errors.
async fn fetch_with_retries(
	sse_headers: &CodexHeaders,
	sse_body: Bytes,
	request: &CodexRequest<'_>,
) -> PiResult<HttpResponse> {
	let options = request.options;
	let signal = request.signal();
	let mut last_error: Option<PiError> = None;
	let max_retries = options
		.and_then(|options| options.max_retries)
		.unwrap_or(DEFAULT_MAX_RETRIES);

	let mut attempt = 0.0;
	while attempt <= max_retries {
		if request.aborted() {
			return Err(PiError::new("Request was aborted"));
		}

		let error = match fetch_attempt(sse_headers, &sse_body, attempt, max_retries, request).await {
			Ok(Some(response)) => return Ok(response),
			Ok(None) => {
				attempt += 1.0;
				continue;
			}
			Err(error) => error,
		};
		let (error, retry_delay_exceeded) = match error {
			AttemptError::RetryDelayExceeded(error) => (error, true),
			AttemptError::Other(error) => (error, false),
		};
		if error.name == "AbortError" || error.message == "Request was aborted" {
			return Err(PiError::new("Request was aborted"));
		}
		last_error = Some(error.clone());
		// Network errors are retryable
		if attempt < max_retries && !retry_delay_exceeded && !error.message.contains("usage limit") {
			sleep(BASE_DELAY_MS * 2f64.powf(attempt), signal).await?;
			attempt += 1.0;
			continue;
		}
		return Err(error);
	}

	Err(last_error.unwrap_or_else(|| PiError::new("Failed after retries")))
}

/// One SSE attempt: `Some(response)` when it is `ok`, `None` after waiting to retry a retryable status.
async fn fetch_attempt(
	sse_headers: &CodexHeaders,
	sse_body: &Bytes,
	attempt: f64,
	max_retries: f64,
	request: &CodexRequest<'_>,
) -> Result<Option<HttpResponse>, AttemptError> {
	let (model, options) = (request.model, request.options);
	let signal = request.signal();
	let http_timeout_ms = request.http_timeout_ms;
	let header_timeout_signal = http_timeout_ms
		.filter(|timeout| *timeout > 0.0)
		.map(AbortSignal::timeout);
	let combined_signal = combine_abort_signals(&[signal, header_timeout_signal.as_ref()]);
	let mut http_request = HttpRequest::new("POST", resolve_codex_url(&model.base_url));
	http_request.headers = sse_headers.record();
	http_request.body = Some(sse_body.clone());
	http_request.signal = combined_signal.signal.clone();
	let fetched = pi_host::fetch(http_request, options.and_then(|options| options.fetch.as_ref())).await;
	combined_signal.cleanup();
	let response = match fetched {
		Ok(response) => response,
		Err(error) => {
			if header_timeout_signal.as_ref().is_some_and(AbortSignal::aborted) && !request.aborted() {
				return Err(AttemptError::Other(PiError::new(format!(
					"Codex SSE response headers timed out after {}ms",
					number_to_string(http_timeout_ms.unwrap_or_default())
				))));
			}
			return Err(AttemptError::Other(error));
		}
	};
	if let Some(on_response) = options.and_then(|options| options.on_response.as_ref()) {
		on_response(
			ProviderResponse {
				status: response.status,
				headers: headers_to_record(&response.headers),
			},
			model,
		)
		.await
		.map_err(AttemptError::Other)?;
	}

	if response.ok() {
		return Ok(Some(response));
	}

	let (status, status_text, headers) = (response.status, response.status_text.clone(), response.headers.clone());
	let error_text = response.text().await.map_err(AttemptError::Other)?;
	if attempt < max_retries && is_retryable_error(status, &error_text) {
		let delay_ms = match get_retry_after_delay_ms(&headers) {
			None => BASE_DELAY_MS * 2f64.powf(attempt),
			Some(retry_after_delay_ms) => validate_retry_delay_ms(retry_after_delay_ms, options)?,
		};

		sleep(delay_ms, signal).await.map_err(AttemptError::Other)?;
		return Ok(None);
	}

	// Parse error for friendly message on final attempt or non-retryable error
	let info = parse_error_response(&error_text, status, &status_text);
	Err(AttemptError::Other(codex_api_error(
		info.friendly_message.unwrap_or(info.message),
		info.code,
		info.payload,
		Some(status),
		Some(headers_to_record(&headers)),
	)))
}

pub fn stream_simple(
	model: &Model,
	context: &TranscriptContext,
	options: Option<SimpleStreamOptions>,
) -> PiResult<AssistantMessageEventStream> {
	let Some(api_key) = options
		.as_ref()
		.and_then(|options| options.api_key.as_deref())
		.filter(|api_key| !api_key.is_empty())
	else {
		return Err(PiError::new(format!("No API key for provider: {}", model.provider)));
	};

	let base = build_base_options(model, context, options.as_ref(), Some(api_key));
	let tool_choice = options
		.as_ref()
		.and_then(|options| options.tool_choice)
		.map(|tool_choice| match tool_choice {
			ToolChoice::Auto => json!("auto"),
			ToolChoice::None => json!("none"),
		});
	let clamped_reasoning = options
		.as_ref()
		.and_then(|options| options.reasoning)
		.map(|reasoning| clamp_thinking_level(model, reasoning.into()));
	let reasoning_effort = clamped_reasoning
		.filter(|level| *level != ModelThinkingLevel::Off)
		.map(|level| level.as_str().to_string());

	Ok(stream(
		model,
		context,
		Some(OpenAICodexResponsesOptions {
			base,
			tool_choice,
			reasoning_effort,
			..OpenAICodexResponsesOptions::default()
		}),
	))
}

// ============================================================================
// Request Building
// ============================================================================

fn build_request_body(
	model: &Model,
	context: &TranscriptContext,
	options: Option<&OpenAICodexResponsesOptions>,
	cache_session_id: Option<&str>,
	grammar_tool_input_properties: &IndexMap<String, String>,
) -> PiResult<Value> {
	let compat = model.openai_responses_compat();
	let supports_strict_mode = compat.supports_strict_mode.unwrap_or(true);
	let supports_openai_grammar_tools = compat.supports_openai_grammar_tools.unwrap_or(false);
	let supports_additional_tools = compat.supports_additional_tools.unwrap_or(false);
	let supports_tool_search = compat.supports_tool_search.unwrap_or(false);
	let supports_mid_convo_system_messages = compat.supports_mid_convo_system_messages.unwrap_or(false);
	let tool_options = ConvertResponsesToolsOptions {
		strict: Some(None),
		supports_strict_mode: Some(supports_strict_mode),
		supports_openai_grammar_tools: Some(supports_openai_grammar_tools),
		..ConvertResponsesToolsOptions::default()
	};
	let transcript_tools = resolve_transcript_tools(
		&context.messages,
		get_responses_tool_support(
			Some(supports_mid_convo_system_messages),
			Some(supports_additional_tools),
			Some(supports_tool_search),
		),
	);
	let messages = convert_responses_messages(
		model,
		context,
		&CODEX_TOOL_CALL_PROVIDERS,
		Some(&ConvertResponsesMessagesOptions {
			include_system_prompt: Some(false),
			grammar_tool_input_properties: Some(grammar_tool_input_properties.clone()),
			supports_mid_convo_system_messages: Some(supports_mid_convo_system_messages),
			supports_additional_tools: Some(supports_additional_tools),
			supports_tool_search: Some(supports_tool_search),
			tool_options: Some(tool_options.clone()),
			..ConvertResponsesMessagesOptions::default()
		}),
	)?;

	let instructions = get_initial_system_message(&context.messages)
		.map(get_system_message_text)
		.unwrap_or_default();
	let text_verbosity = options
		.and_then(|options| options.text_verbosity.as_ref())
		.filter(|verbosity| truthy(Some(verbosity)))
		.cloned()
		.unwrap_or_else(|| json!("low"));
	let mut body = Map::new();
	body.insert("model".into(), json!(model.id));
	body.insert("store".into(), json!(false));
	body.insert("stream".into(), json!(true));
	body.insert(
		"instructions".into(),
		json!(if instructions.is_empty() {
			"You are a helpful assistant."
		} else {
			&instructions
		}),
	);
	body.insert("input".into(), Value::Array(messages));
	body.insert("text".into(), json!({ "verbosity": text_verbosity }));
	body.insert("include".into(), json!(["reasoning.encrypted_content"]));
	if let Some(cache_session_id) = cache_session_id {
		body.insert("prompt_cache_key".into(), json!(cache_session_id));
	}
	body.insert(
		"tool_choice".into(),
		options
			.and_then(|options| options.tool_choice.clone())
			.filter(|tool_choice| !tool_choice.is_null())
			.unwrap_or_else(|| json!("auto")),
	);
	body.insert("parallel_tool_calls".into(), json!(true));

	if let Some(temperature) = options.and_then(|options| options.temperature) {
		body.insert("temperature".into(), json!(temperature));
	}

	if let Some(service_tier) = options.and_then(|options| options.service_tier.as_ref()) {
		body.insert("service_tier".into(), service_tier.clone());
	}

	if !transcript_tools.request_tools.is_empty() {
		body.insert(
			"tools".into(),
			Value::Array(convert_responses_tools(
				&transcript_tools.request_tools,
				Some(&tool_options),
			)?),
		);
	}

	let thinking_level_map = model.thinking_level_map.as_ref();
	if let Some(reasoning_effort) = options.and_then(|options| options.reasoning_effort.as_deref()) {
		let effort = if reasoning_effort == "none" {
			match thinking_level_map.and_then(|map| map.get(ModelThinkingLevel::Off)) {
				None => Some("none".to_string()),
				Some(off) => off.map(str::to_string),
			}
		} else {
			let mapped = serde_json::from_value::<ModelThinkingLevel>(json!(reasoning_effort))
				.ok()
				.and_then(|level| thinking_level_map.and_then(|map| map.get(level)))
				.flatten();
			Some(mapped.unwrap_or(reasoning_effort).to_string())
		};
		if let Some(effort) = effort {
			let summary = options
				.and_then(|options| options.reasoning_summary.clone())
				.filter(|summary| !summary.is_null())
				.unwrap_or_else(|| json!("auto"));
			body.insert("reasoning".into(), json!({ "effort": effort, "summary": summary }));
		}
	} else {
		let off = thinking_level_map.and_then(|map| map.get(ModelThinkingLevel::Off));
		if model.reasoning && off != Some(None) {
			body.insert("reasoning".into(), json!({ "effort": off.flatten().unwrap_or("none") }));
		}
	}

	Ok(Value::Object(body))
}

fn get_service_tier_cost_multiplier(model_id: &str, service_tier: Option<&str>) -> f64 {
	match service_tier {
		Some("flex") => 0.5,
		Some("priority") => {
			if model_id == "gpt-5.5" {
				2.5
			} else {
				2.0
			}
		}
		_ => 1.0,
	}
}

fn apply_service_tier_pricing(usage: &mut Usage, service_tier: Option<&str>, model_id: &str) {
	let multiplier = get_service_tier_cost_multiplier(model_id, service_tier);
	if multiplier == 1.0 {
		return;
	}

	usage.cost.input *= multiplier;
	usage.cost.output *= multiplier;
	usage.cost.cache_read *= multiplier;
	usage.cost.cache_write *= multiplier;
	usage.cost.total = usage.cost.input + usage.cost.output + usage.cost.cache_read + usage.cost.cache_write;
}

fn resolve_codex_service_tier(
	response_service_tier: Option<&str>,
	request_service_tier: Option<&str>,
) -> Option<String> {
	if response_service_tier == Some("default") && matches!(request_service_tier, Some("flex" | "priority")) {
		return request_service_tier.map(str::to_string);
	}
	response_service_tier.or(request_service_tier).map(str::to_string)
}

fn resolve_codex_url(base_url: &str) -> String {
	let raw = if !js_trim(base_url).is_empty() {
		base_url
	} else {
		DEFAULT_CODEX_BASE_URL
	};
	let normalized = raw.trim_end_matches('/');
	if normalized.ends_with("/codex/responses") {
		return normalized.to_string();
	}
	if normalized.ends_with("/codex") {
		return format!("{normalized}/responses");
	}
	format!("{normalized}/codex/responses")
}

fn resolve_codex_websocket_url(base_url: &str) -> PiResult<String> {
	let mut url = url::Url::parse(&resolve_codex_url(base_url)).map_err(|_| PiError::type_error("Invalid URL"))?;
	let scheme = match url.scheme() {
		"https" => Some("wss"),
		"http" => Some("ws"),
		_ => None,
	};
	if let Some(scheme) = scheme {
		let _ = url.set_scheme(scheme);
	}
	Ok(url.to_string())
}

// ============================================================================
// Response Processing
// ============================================================================

fn responses_stream_options(request: &CodexRequest<'_>) -> OpenAIResponsesStreamOptions {
	let options = request.options;
	let model_id = request.model.id.clone();
	OpenAIResponsesStreamOptions {
		service_tier: options
			.and_then(|options| options.service_tier.as_ref())
			.and_then(Value::as_str)
			.map(str::to_string),
		grammar_tool_input_properties: Some(request.grammar_tool_input_properties.clone()),
		on_image_generation_call: options.and_then(|options| options.on_image_generation_call.clone()),
		on_image_generation_partial_image: options
			.and_then(|options| options.on_image_generation_partial_image.clone()),
		resolve_service_tier: Some(Rc::new(resolve_codex_service_tier)),
		apply_service_tier_pricing: Some(Rc::new(move |usage: &mut Usage, service_tier: Option<&str>| {
			apply_service_tier_pricing(usage, service_tier, &model_id)
		})),
		..OpenAIResponsesStreamOptions::default()
	}
}

async fn process_stream(response: HttpResponse, request: &CodexRequest<'_>) -> PiResult<()> {
	let events = map_codex_events(
		parse_sse(response, request.signal().cloned()),
		request.output.clone(),
		request.model.clone(),
		request
			.options
			.and_then(|options| options.on_provider_stream_event.clone()),
	);
	process_responses_stream(
		events,
		request.output,
		request.stream,
		request.model,
		Some(&responses_stream_options(request)),
	)
	.await
}

/// `new CodexApiError(message, { code, payload, status, headers })`.
fn codex_api_error(
	message: String,
	code: Option<Value>,
	payload: Option<Value>,
	status: Option<u16>,
	headers: Option<IndexMap<String, String>>,
) -> PiError {
	let mut data = Map::new();
	if let Some(code) = code {
		data.insert("code".into(), code);
	}
	if let Some(payload) = payload {
		data.insert("payload".into(), payload);
	}
	if let Some(headers) = headers {
		data.insert(
			"headers".into(),
			Value::Object(
				headers
					.into_iter()
					.map(|(name, value)| (name, Value::String(value)))
					.collect(),
			),
		);
	}
	PiError {
		name: "CodexApiError".into(),
		message,
		status,
		data: Some(Value::Object(data)),
	}
}

/// `new CodexProtocolError(message, { payload, cause })`.
fn codex_protocol_error(message: String, payload: Value) -> PiError {
	PiError {
		name: "CodexProtocolError".into(),
		message,
		status: None,
		data: Some(json!({ "payload": payload })),
	}
}

/// `new ProviderStreamEventCallbackError(cause)`.
fn provider_stream_event_callback_error(cause: &PiError) -> PiError {
	PiError::named("ProviderStreamEventCallbackError", format_thrown_value(cause))
}

fn is_codex_non_transport_error(error: &PiError) -> bool {
	matches!(
		error.name.as_str(),
		"CodexApiError" | "CodexProtocolError" | "ProviderStreamEventCallbackError"
	)
}

fn codex_api_error_code(error: &PiError) -> Option<&str> {
	if error.name != "CodexApiError" {
		return None;
	}
	error.data.as_ref()?.get("code")?.as_str()
}

fn is_websocket_connection_limit_reached_error(error: &PiError) -> bool {
	codex_api_error_code(error) == Some(WEBSOCKET_CONNECTION_LIMIT_REACHED_CODE)
}

fn is_previous_response_not_found_error(error: &PiError) -> bool {
	codex_api_error_code(error) == Some(PREVIOUS_RESPONSE_NOT_FOUND_CODE)
}

fn extract_codex_event_error(event: &Value) -> (Option<String>, Option<String>) {
	let nested = event.get("error").filter(|error| error.is_object());
	let string_field = |name: &str| {
		event
			.get(name)
			.and_then(Value::as_str)
			.or_else(|| nested.and_then(|nested| nested.get(name)).and_then(Value::as_str))
			.map(str::to_string)
	};
	(string_field("code"), string_field("message"))
}

/// `TypeError: Cannot read properties of null (reading 'type')`.
fn null_event_type_error() -> PiError {
	PiError::type_error("Cannot read properties of null (reading 'type')")
}

/// `mapCodexEvents`: calls the event observer, fails on error events and ends at the terminal response event, which
/// it hands on as `response.completed`.
fn map_codex_events(
	events: LocalBoxStream<'static, PiResult<Value>>,
	output: SharedAssistantMessage,
	model: Model,
	on_provider_stream_event: Option<OnProviderStreamEvent>,
) -> LocalBoxStream<'static, PiResult<Value>> {
	let state = Some((events, output, model, on_provider_stream_event));
	futures::stream::unfold(state, |state| async move {
		let (mut events, output, model, on_provider_stream_event) = state?;
		loop {
			let event = match events.next().await? {
				Ok(event) => event,
				Err(error) => return Some((Err(error), None)),
			};
			if let Some(on_provider_stream_event) = &on_provider_stream_event
				&& let Err(error) = on_provider_stream_event(&event, &model).await
			{
				// Keep callback failures out of Codex's WebSocket retry and SSE fallback path.
				return Some((Err(provider_stream_event_callback_error(&error)), None));
			}
			if event.is_null() {
				return Some((Err(null_event_type_error()), None));
			}
			let Some(event_type) = event
				.get("type")
				.and_then(Value::as_str)
				.filter(|event_type| !event_type.is_empty())
			else {
				continue;
			};

			if event_type == "error" {
				let (code, message) = extract_codex_event_error(&event);
				let text = message
					.filter(|message| !message.is_empty())
					.or_else(|| code.clone().filter(|code| !code.is_empty()))
					.unwrap_or_else(|| json_stringify(&event));
				let error = codex_api_error(
					format!("Codex error: {text}"),
					code.map(Value::String),
					Some(event),
					None,
					None,
				);
				return Some((Err(error), None));
			}

			if event_type == "response.failed" {
				let error = event.get("response").and_then(|response| response.get("error"));
				let code = error.and_then(|error| error.get("code")).cloned();
				let message = error
					.and_then(|error| error.get("message"))
					.filter(|message| truthy(Some(message)))
					.map(|message| js_string(Some(message)))
					.unwrap_or_else(|| "Codex response failed".into());
				return Some((Err(codex_api_error(message, code, Some(event), None, None)), None));
			}

			if matches!(
				event_type,
				"response.done" | "response.completed" | "response.incomplete"
			) {
				return Some((Ok(completed_event(event, &output)), None));
			}

			return Some((Ok(event), Some((events, output, model, on_provider_stream_event))));
		}
	})
	.boxed_local()
}

/// `{ ...event, type: "response.completed", response: { ...response, status: normalizeCodexStatus(status) } }`, after
/// recording `response.end_turn`.
fn completed_event(event: Value, output: &SharedAssistantMessage) -> Value {
	let response = event.get("response");
	if let Some(end_turn) = response
		.and_then(|response| response.get("end_turn"))
		.and_then(Value::as_bool)
	{
		output.borrow_mut().end_turn = Some(end_turn);
	}
	let normalized_response = match response {
		Some(Value::Object(response)) => {
			let mut response = response.clone();
			match normalize_codex_status(response.get("status")) {
				Some(status) => {
					response.insert("status".into(), json!(status));
				}
				None => {
					response.shift_remove("status");
				}
			}
			Some(Value::Object(response))
		}
		other => other.cloned(),
	};
	let mut completed = match event {
		Value::Object(event) => event,
		_ => Map::new(),
	};
	completed.insert("type".into(), json!("response.completed"));
	match normalized_response {
		Some(response) => {
			completed.insert("response".into(), response);
		}
		None => {
			completed.shift_remove("response");
		}
	}
	Value::Object(completed)
}

fn normalize_codex_status(status: Option<&Value>) -> Option<&'static str> {
	let status = status?.as_str()?;
	CODEX_RESPONSE_STATUSES.iter().find(|known| **known == status).copied()
}

// ============================================================================
// SSE Parsing
// ============================================================================

/// `TextDecoder` (UTF-8, not fatal, BOM removed) fed with `decode(chunk, { stream: true })`.
#[derive(Default)]
struct Utf8StreamDecoder {
	pending: Vec<u8>,
	started: bool,
}

impl Utf8StreamDecoder {
	fn decode(&mut self, chunk: &[u8]) -> String {
		self.pending.extend_from_slice(chunk);
		let mut text = String::new();
		let mut input: &[u8] = &self.pending;
		loop {
			match std::str::from_utf8(input) {
				Ok(valid) => {
					text.push_str(valid);
					input = &[];
					break;
				}
				Err(error) => {
					let (valid, rest) = input.split_at(error.valid_up_to());
					text.push_str(std::str::from_utf8(valid).unwrap_or_default());
					match error.error_len() {
						Some(length) => {
							text.push('\u{FFFD}');
							input = &rest[length..];
						}
						None => {
							input = rest;
							break;
						}
					}
				}
			}
		}
		self.pending = input.to_vec();
		self.strip_bom(text)
	}

	/// `decoder.decode()`: flushes an incomplete sequence as U+FFFD.
	fn finish(&mut self) -> String {
		let text = String::from_utf8_lossy(&self.pending).into_owned();
		self.pending.clear();
		self.strip_bom(text)
	}

	fn strip_bom(&mut self, text: String) -> String {
		if self.started || text.is_empty() {
			return text;
		}
		self.started = true;
		match text.strip_prefix('\u{FEFF}') {
			Some(rest) => rest.to_string(),
			None => text,
		}
	}
}

/// `parseSSE`'s generator state.
struct SseParser {
	body: pi_host::Body,
	signal: Option<AbortSignal>,
	decoder: Utf8StreamDecoder,
	buffer: String,
	done: bool,
}

impl SseParser {
	async fn next_event(&mut self) -> PiResult<Option<Value>> {
		loop {
			while let Some(index) = self.buffer.find("\n\n") {
				let chunk = self.buffer[..index].to_string();
				self.buffer.drain(..index + 2);

				let data_lines: Vec<&str> = chunk
					.split('\n')
					.filter(|line| line.starts_with("data:"))
					.map(|line| js_trim(&line[5..]))
					.collect();
				if data_lines.is_empty() {
					continue;
				}
				let data = js_trim(&data_lines.join("\n")).to_string();
				if data.is_empty() || data == "[DONE]" {
					continue;
				}
				return match json_parse(&data) {
					Ok(event) => Ok(Some(event)),
					Err(cause) => Err(codex_protocol_error(
						format!("Invalid Codex SSE JSON: {}", format_thrown_value(&cause)),
						Value::String(data),
					)),
				};
			}

			if self.done {
				return Ok(None);
			}
			if self.aborted() {
				return Err(PiError::new("Request was aborted"));
			}
			let chunk = match self.signal.clone() {
				Some(signal) => {
					let aborted = signal.aborted_future();
					futures::pin_mut!(aborted);
					match select(self.body.next(), aborted).await {
						Either::Left((chunk, _)) => chunk,
						// The TS abort listener cancels the reader, so the pending read ends.
						Either::Right(_) => None,
					}
				}
				None => self.body.next().await,
			};
			if self.aborted() {
				return Err(PiError::new("Request was aborted"));
			}
			match chunk {
				Some(Ok(bytes)) => {
					let text = self.decoder.decode(&bytes);
					self.buffer.push_str(&text);
				}
				Some(Err(error)) => return Err(error),
				None => {
					let text = self.decoder.finish();
					self.buffer.push_str(&text);
					// Treat EOF as terminating the residual SSE frame.
					if !js_trim(&self.buffer).is_empty() {
						self.buffer.push_str("\n\n");
					}
					self.done = true;
				}
			}
		}
	}

	fn aborted(&self) -> bool {
		self.signal.as_ref().is_some_and(AbortSignal::aborted)
	}
}

fn parse_sse(response: HttpResponse, signal: Option<AbortSignal>) -> LocalBoxStream<'static, PiResult<Value>> {
	let parser = SseParser {
		body: response.body,
		signal,
		decoder: Utf8StreamDecoder::default(),
		buffer: String::new(),
		done: false,
	};
	futures::stream::unfold(Some(parser), |parser| async move {
		let mut parser = parser?;
		match parser.next_event().await {
			Ok(Some(event)) => Some((Ok(event), Some(parser))),
			Ok(None) => None,
			Err(error) => Some((Err(error), None)),
		}
	})
	.boxed_local()
}

// ============================================================================
// WebSocket Parsing
// ============================================================================

const OPENAI_BETA_RESPONSES_WEBSOCKETS: &str = "responses_websockets=2026-02-06";
const SESSION_WEBSOCKET_CACHE_TTL_MS: f64 = 5.0 * 60.0 * 1000.0;
const SESSION_WEBSOCKET_MAX_AGE_MS: f64 = 55.0 * 60.0 * 1000.0;

struct CachedWebSocketContinuationState {
	last_request_body: Value,
	last_response_id: String,
	last_response_items: Vec<Value>,
}

struct CachedWebSocketConnection {
	socket: Rc<dyn WebSocket>,
	busy: Cell<bool>,
	created_at: f64,
	idle_timer: RefCell<Option<AbortController>>,
	continuation: RefCell<Option<CachedWebSocketContinuationState>>,
}

/// Per-session WebSocket counters for debugging.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct OpenAICodexWebSocketDebugStats {
	pub requests: u64,
	pub connections_created: u64,
	pub connections_reused: u64,
	pub cached_context_requests: u64,
	pub store_true_requests: u64,
	pub full_context_requests: u64,
	pub delta_requests: u64,
	pub last_input_items: u64,
	pub last_delta_input_items: Option<u64>,
	pub last_previous_response_id: Option<String>,
	pub websocket_failures: u64,
	pub sse_fallbacks: u64,
	pub websocket_fallback_active: Option<bool>,
	pub last_websocket_error: Option<String>,
}

type SessionCache = IndexMap<String, IndexMap<String, Rc<CachedWebSocketConnection>>>;

thread_local! {
	static WEBSOCKET_SESSION_CACHE: RefCell<SessionCache> = RefCell::default();
	static WEBSOCKET_DEBUG_STATS: RefCell<HashMap<String, OpenAICodexWebSocketDebugStats>> = RefCell::default();
	static WEBSOCKET_SSE_FALLBACK_SESSIONS: RefCell<HashSet<String>> = RefCell::default();
	static SESSION_RESOURCE_CLEANUP_REGISTERED: Cell<bool> = const { Cell::new(false) };
}

/// `getOrCreateWebSocketDebugStats(sessionId)`, handed to `update`.
fn with_websocket_debug_stats<T>(session_id: &str, update: impl FnOnce(&mut OpenAICodexWebSocketDebugStats) -> T) -> T {
	WEBSOCKET_DEBUG_STATS.with(|stats| update(stats.borrow_mut().entry(session_id.to_string()).or_default()))
}

/// A copy of the WebSocket counters of `session_id`.
pub fn get_openai_codex_websocket_debug_stats(session_id: &str) -> Option<OpenAICodexWebSocketDebugStats> {
	WEBSOCKET_DEBUG_STATS.with(|stats| stats.borrow().get(session_id).cloned())
}

/// Forgets the counters and the SSE fallback of `session_id`, or of every session.
pub fn reset_openai_codex_websocket_debug_stats(session_id: Option<&str>) {
	if let Some(session_id) = session_id.filter(|session_id| !session_id.is_empty()) {
		WEBSOCKET_DEBUG_STATS.with(|stats| stats.borrow_mut().remove(session_id));
		WEBSOCKET_SSE_FALLBACK_SESSIONS.with(|sessions| sessions.borrow_mut().remove(session_id));
		return;
	}
	WEBSOCKET_DEBUG_STATS.with(|stats| stats.borrow_mut().clear());
	WEBSOCKET_SSE_FALLBACK_SESSIONS.with(|sessions| sessions.borrow_mut().clear());
}

/// Closes the cached WebSockets of `session_id`, or of every session.
pub fn close_openai_codex_websocket_sessions(session_id: Option<&str>) {
	let close_entry = |entry: &CachedWebSocketConnection| {
		clear_idle_timer(entry);
		close_websocket_silently(&*entry.socket, 1000, "debug_close");
	};
	if let Some(session_id) = session_id.filter(|session_id| !session_id.is_empty()) {
		let entries = WEBSOCKET_SESSION_CACHE.with(|cache| cache.borrow().get(session_id).cloned());
		for entry in entries.iter().flat_map(IndexMap::values) {
			close_entry(entry);
		}
		WEBSOCKET_SESSION_CACHE.with(|cache| cache.borrow_mut().shift_remove(session_id));
		return;
	}
	let sessions = WEBSOCKET_SESSION_CACHE.with(|cache| cache.borrow().clone());
	for entry in sessions.values().flat_map(IndexMap::values) {
		close_entry(entry);
	}
	WEBSOCKET_SESSION_CACHE.with(|cache| cache.borrow_mut().clear());
}

fn ensure_session_resource_cleanup_registered() {
	if SESSION_RESOURCE_CLEANUP_REGISTERED.with(|registered| registered.replace(true)) {
		return;
	}
	// Registered for the life of the thread, like the TS module-level registration.
	let _unregister = register_session_resource_cleanup(Rc::new(|session_id| {
		close_openai_codex_websocket_sessions(session_id);
		Ok(())
	}));
}

fn is_websocket_sse_fallback_active(session_id: Option<&str>) -> bool {
	match session_id.filter(|session_id| !session_id.is_empty()) {
		Some(session_id) => WEBSOCKET_SSE_FALLBACK_SESSIONS.with(|sessions| sessions.borrow().contains(session_id)),
		None => false,
	}
}

fn record_websocket_sse_fallback(session_id: Option<&str>) {
	let Some(session_id) = session_id.filter(|session_id| !session_id.is_empty()) else {
		return;
	};
	let active = is_websocket_sse_fallback_active(Some(session_id));
	with_websocket_debug_stats(session_id, |stats| {
		stats.sse_fallbacks += 1;
		stats.websocket_fallback_active = Some(active);
	});
}

fn record_websocket_failure(session_id: Option<&str>, error: &PiError) {
	let Some(session_id) = session_id.filter(|session_id| !session_id.is_empty()) else {
		return;
	};
	WEBSOCKET_SSE_FALLBACK_SESSIONS.with(|sessions| sessions.borrow_mut().insert(session_id.to_string()));

	with_websocket_debug_stats(session_id, |stats| {
		stats.websocket_failures += 1;
		stats.last_websocket_error = Some(format_thrown_value(error));
		stats.websocket_fallback_active = Some(true);
	});
}

/// `new WebSocketCloseError(message, { code, reason, wasClean })`.
fn websocket_close_error(message: String, code: u16, reason: &str, was_clean: bool) -> PiError {
	let mut data = Map::new();
	data.insert("code".into(), json!(code));
	if !reason.is_empty() {
		data.insert("reason".into(), json!(reason));
	}
	data.insert("wasClean".into(), json!(was_clean));
	PiError {
		name: "WebSocketCloseError".into(),
		message,
		status: None,
		data: Some(Value::Object(data)),
	}
}

fn is_websocket_reusable(socket: &dyn WebSocket) -> bool {
	// If readyState is unavailable, assume the runtime keeps it open/reusable.
	socket.ready_state().is_none_or(|ready_state| ready_state == OPEN)
}

fn is_websocket_session_expired(entry: &CachedWebSocketConnection) -> bool {
	pi_host::now_ms() - entry.created_at >= SESSION_WEBSOCKET_MAX_AGE_MS
}

fn close_websocket_silently(socket: &dyn WebSocket, code: u16, reason: &str) {
	let _ = socket.close(code, reason);
}

fn clear_idle_timer(entry: &CachedWebSocketConnection) {
	let timer = entry.idle_timer.borrow_mut().take();
	if let Some(timer) = timer {
		timer.abort();
	}
}

/// Removes the entry of `account_id` (only while it is `only`, when given), and the session once it has none.
fn remove_cached_websocket(session_id: &str, account_id: &str, only: Option<&Rc<CachedWebSocketConnection>>) {
	WEBSOCKET_SESSION_CACHE.with(|cache| {
		let mut cache = cache.borrow_mut();
		let Some(account_entries) = cache.get_mut(session_id) else {
			return;
		};
		let matches = match (only, account_entries.get(account_id)) {
			(None, _) => true,
			(Some(entry), Some(current)) => Rc::ptr_eq(entry, current),
			(Some(_), None) => false,
		};
		if matches {
			account_entries.shift_remove(account_id);
		}
		if account_entries.is_empty() {
			cache.shift_remove(session_id);
		}
	});
}

fn schedule_session_websocket_expiry(session_id: &str, account_id: &str, entry: &Rc<CachedWebSocketConnection>) {
	clear_idle_timer(entry);
	let timer = AbortController::new();
	let signal = timer.signal();
	*entry.idle_timer.borrow_mut() = Some(timer);
	let (session_id, account_id, entry) = (session_id.to_string(), account_id.to_string(), entry.clone());
	pi_host::spawn_local(async move {
		if pi_host::sleep(SESSION_WEBSOCKET_CACHE_TTL_MS, Some(&signal))
			.await
			.is_err()
		{
			return;
		}
		if entry.busy.get() {
			return;
		}
		close_websocket_silently(&*entry.socket, 1000, "idle_timeout");
		remove_cached_websocket(&session_id, &account_id, Some(&entry));
	});
}

/// Settles a pending `connectWebSocket` promise, closing the socket with the reason when it fails.
type SettleConnect = Rc<dyn Fn(PiResult<()>, Option<&str>)>;

/// The state of a pending `connectWebSocket` promise.
struct PendingConnect {
	settled: bool,
	resolve: Option<oneshot::Sender<PiResult<()>>>,
	listener: Option<WebSocketListenerId>,
	abort_listener: Option<ListenerId>,
	timeout: Option<AbortController>,
}

async fn connect_websocket(
	url: &str,
	headers: &CodexHeaders,
	signal: Option<&AbortSignal>,
	connect_timeout_ms: Option<f64>,
) -> PiResult<Rc<dyn WebSocket>> {
	let connect_timeout_ms = connect_timeout_ms.unwrap_or(DEFAULT_WEBSOCKET_CONNECT_TIMEOUT_MS);
	let mut ws_headers = headers.record();
	// A no-op, as in the TS: `headersToRecord` names are lowercase.
	ws_headers.shift_remove("OpenAI-Beta");

	let socket = match pi_host::websocket::connect(url, &ws_headers) {
		None => return Err(PiError::new("WebSocket transport is not available in this runtime")),
		Some(socket) => socket?,
	};

	let (resolve, settled) = oneshot::channel();
	let pending = Rc::new(RefCell::new(PendingConnect {
		settled: false,
		resolve: Some(resolve),
		listener: None,
		abort_listener: None,
		timeout: None,
	}));
	let finish: SettleConnect = {
		let (pending, socket, signal) = (pending.clone(), socket.clone(), signal.cloned());
		Rc::new(move |result, close_reason| {
			let (resolve, listener, abort_listener, timeout) = {
				let mut pending = pending.borrow_mut();
				if pending.settled {
					return;
				}
				pending.settled = true;
				(
					pending.resolve.take(),
					pending.listener.take(),
					pending.abort_listener.take(),
					pending.timeout.take(),
				)
			};
			if let Some(timeout) = timeout {
				timeout.abort();
			}
			if let Some(listener) = listener {
				socket.remove_event_listener(listener);
			}
			if let (Some(signal), Some(abort_listener)) = (&signal, abort_listener) {
				signal.remove_listener(abort_listener);
			}
			if let (Err(_), Some(close_reason)) = (&result, close_reason) {
				close_websocket_silently(&*socket, 1000, close_reason);
			}
			if let Some(resolve) = resolve {
				let _ = resolve.send(result);
			}
		})
	};

	let listener = {
		let finish = finish.clone();
		socket.add_event_listener(Rc::new(move |event: &WebSocketEvent| match event {
			WebSocketEvent::Open => finish(Ok(()), None),
			WebSocketEvent::Error { message } => finish(Err(extract_websocket_error(message.as_deref())), None),
			WebSocketEvent::Close {
				code,
				reason,
				was_clean,
			} => finish(Err(extract_websocket_close_error(*code, reason, *was_clean)), None),
			WebSocketEvent::Message { .. } => {}
		}))
	};
	pending.borrow_mut().listener = Some(listener);
	let on_abort = {
		let finish = finish.clone();
		Rc::new(move || finish(Err(PiError::new("Request was aborted")), Some("aborted")))
	};
	if let Some(signal) = signal {
		let on_abort = on_abort.clone();
		let abort_listener = signal.add_listener(move |_| on_abort());
		pending.borrow_mut().abort_listener = Some(abort_listener);
	}

	if connect_timeout_ms > 0.0 {
		let timeout = AbortController::new();
		let timeout_signal = timeout.signal();
		pending.borrow_mut().timeout = Some(timeout);
		let finish = finish.clone();
		pi_host::spawn_local(async move {
			if pi_host::sleep(connect_timeout_ms, Some(&timeout_signal)).await.is_ok() {
				finish(
					Err(PiError::new(format!(
						"WebSocket connect timeout after {}ms",
						number_to_string(connect_timeout_ms)
					))),
					Some("connect_timeout"),
				);
			}
		});
	}
	if signal.is_some_and(AbortSignal::aborted) {
		on_abort();
	}

	match settled.await {
		Ok(result) => result.map(|()| socket),
		Err(_) => futures::future::pending().await,
	}
}

/// A socket from `acquireWebSocket` and its `release({ keep })`.
struct AcquiredWebSocket {
	socket: Rc<dyn WebSocket>,
	entry: Option<Rc<CachedWebSocketConnection>>,
	reused: bool,
	release: Box<dyn FnOnce(bool)>,
}

/// A connection outside the session cache: `release` always closes it.
fn uncached_websocket(socket: Rc<dyn WebSocket>) -> AcquiredWebSocket {
	AcquiredWebSocket {
		socket: socket.clone(),
		entry: None,
		reused: false,
		release: Box::new(move |_keep| close_websocket_silently(&*socket, 1000, "done")),
	}
}

async fn acquire_websocket(
	url: &str,
	headers: &CodexHeaders,
	session_id: Option<&str>,
	account_id: &str,
	signal: Option<&AbortSignal>,
	connect_timeout_ms: Option<f64>,
) -> PiResult<AcquiredWebSocket> {
	let Some(session_id) = session_id.filter(|session_id| !session_id.is_empty()) else {
		let socket = connect_websocket(url, headers, signal, connect_timeout_ms).await?;
		return Ok(uncached_websocket(socket));
	};

	let cached = WEBSOCKET_SESSION_CACHE.with(|cache| {
		cache
			.borrow()
			.get(session_id)
			.and_then(|entries| entries.get(account_id))
			.cloned()
	});
	if let Some(cached) = cached {
		clear_idle_timer(&cached);
		if !cached.busy.get() && is_websocket_session_expired(&cached) {
			close_websocket_silently(&*cached.socket, 1000, "connection_age_limit");
			remove_cached_websocket(session_id, account_id, None);
		} else if !cached.busy.get() && is_websocket_reusable(&*cached.socket) {
			cached.busy.set(true);
			let (session_id, account_id, entry) = (session_id.to_string(), account_id.to_string(), cached.clone());
			return Ok(AcquiredWebSocket {
				socket: cached.socket.clone(),
				entry: Some(cached),
				reused: true,
				release: Box::new(move |keep| {
					if !keep || !is_websocket_reusable(&*entry.socket) {
						close_websocket_silently(&*entry.socket, 1000, "done");
						remove_cached_websocket(&session_id, &account_id, Some(&entry));
						return;
					}
					entry.busy.set(false);
					schedule_session_websocket_expiry(&session_id, &account_id, &entry);
				}),
			});
		} else if cached.busy.get() {
			let socket = connect_websocket(url, headers, signal, connect_timeout_ms).await?;
			return Ok(uncached_websocket(socket));
		} else {
			close_websocket_silently(&*cached.socket, 1000, "done");
			remove_cached_websocket(session_id, account_id, None);
		}
	}

	let socket = connect_websocket(url, headers, signal, connect_timeout_ms).await?;
	let entry = Rc::new(CachedWebSocketConnection {
		socket: socket.clone(),
		busy: Cell::new(true),
		created_at: pi_host::now_ms(),
		idle_timer: RefCell::new(None),
		continuation: RefCell::new(None),
	});
	WEBSOCKET_SESSION_CACHE.with(|cache| {
		cache
			.borrow_mut()
			.entry(session_id.to_string())
			.or_default()
			.insert(account_id.to_string(), entry.clone());
	});
	let (session_id, account_id, cached_entry) = (session_id.to_string(), account_id.to_string(), entry.clone());
	Ok(AcquiredWebSocket {
		socket,
		entry: Some(entry),
		reused: false,
		release: Box::new(move |keep| {
			if !keep || !is_websocket_reusable(&*cached_entry.socket) {
				close_websocket_silently(&*cached_entry.socket, 1000, "done");
				clear_idle_timer(&cached_entry);
				remove_cached_websocket(&session_id, &account_id, Some(&cached_entry));
				return;
			}
			cached_entry.busy.set(false);
			schedule_session_websocket_expiry(&session_id, &account_id, &cached_entry);
		}),
	})
}

fn extract_websocket_error(message: Option<&str>) -> PiError {
	match message.filter(|message| !message.is_empty()) {
		Some(message) => PiError::new(message),
		None => PiError::new("WebSocket error"),
	}
}

fn extract_websocket_close_error(code: u16, reason: &str, was_clean: bool) -> PiError {
	let mut reason_text = if reason.is_empty() {
		String::new()
	} else {
		format!(" {reason}")
	};
	if reason_text.is_empty() && code == WEBSOCKET_MESSAGE_TOO_BIG_CLOSE_CODE {
		reason_text = " message too big".into();
	}
	websocket_close_error(
		js_trim(&format!("WebSocket closed {code}{reason_text}")).to_string(),
		code,
		reason,
		was_clean,
	)
}

fn decode_websocket_data(data: &WebSocketData) -> String {
	match data {
		WebSocketData::Text(text) => text.clone(),
		WebSocketData::Binary(bytes) => {
			let mut decoder = Utf8StreamDecoder::default();
			let mut text = decoder.decode(bytes);
			text.push_str(&decoder.finish());
			text
		}
	}
}

/// The queue `parseWebSocket`'s listeners fill.
#[derive(Default)]
struct WebSocketQueue {
	queue: VecDeque<Value>,
	pending: Option<oneshot::Sender<()>>,
	done: bool,
	failed: Option<PiError>,
	saw_completion: bool,
}

impl WebSocketQueue {
	fn wake(&mut self) {
		if let Some(pending) = self.pending.take() {
			let _ = pending.send(());
		}
	}

	fn fail(&mut self, error: PiError) {
		self.failed = Some(error);
		self.done = true;
		self.wake();
	}

	fn on_event(&mut self, event: &WebSocketEvent) {
		match event {
			WebSocketEvent::Message { data } => self.on_message(decode_websocket_data(data)),
			WebSocketEvent::Error { message } => self.fail(extract_websocket_error(message.as_deref())),
			WebSocketEvent::Close {
				code,
				reason,
				was_clean,
			} => {
				if self.saw_completion {
					self.done = true;
					self.wake();
					return;
				}
				if self.failed.is_none() {
					self.failed = Some(extract_websocket_close_error(*code, reason, *was_clean));
				}
				self.done = true;
				self.wake();
			}
			WebSocketEvent::Open => {}
		}
	}

	fn on_message(&mut self, text: String) {
		if text.is_empty() {
			return;
		}
		let parsed = json_parse(&text).and_then(|parsed| {
			if parsed.is_null() {
				return Err(null_event_type_error());
			}
			Ok(parsed)
		});
		match parsed {
			Ok(parsed) => {
				let event_type = parsed.get("type").and_then(Value::as_str).unwrap_or("");
				if matches!(
					event_type,
					"response.completed" | "response.done" | "response.incomplete"
				) {
					self.saw_completion = true;
					self.done = true;
				}
				self.queue.push_back(parsed);
				self.wake();
			}
			Err(cause) => self.fail(codex_protocol_error(
				format!("Invalid Codex WebSocket JSON: {}", format_thrown_value(&cause)),
				Value::String(text),
			)),
		}
	}
}

/// `parseWebSocket`'s generator: its listeners are added on the first pull and removed when it is dropped.
struct WebSocketParser {
	socket: Rc<dyn WebSocket>,
	signal: Option<AbortSignal>,
	idle_timeout_ms: Option<f64>,
	queue: Rc<RefCell<WebSocketQueue>>,
	listeners: Option<(WebSocketListenerId, Option<ListenerId>)>,
}

impl WebSocketParser {
	fn add_listeners(&mut self) {
		if self.listeners.is_some() {
			return;
		}
		let queue = self.queue.clone();
		let listener = self.socket.add_event_listener(Rc::new(move |event: &WebSocketEvent| {
			queue.borrow_mut().on_event(event)
		}));
		let abort_listener = self.signal.as_ref().map(|signal| {
			let queue = self.queue.clone();
			signal.add_listener(move |_| queue.borrow_mut().fail(PiError::new("Request was aborted")))
		});
		self.listeners = Some((listener, abort_listener));
	}

	async fn next_event(&mut self) -> PiResult<Option<Value>> {
		self.add_listeners();
		loop {
			if self.signal.as_ref().is_some_and(AbortSignal::aborted) {
				return Err(PiError::new("Request was aborted"));
			}
			let receiver = {
				let mut queue = self.queue.borrow_mut();
				if let Some(event) = queue.queue.pop_front() {
					return Ok(Some(event));
				}
				if queue.done {
					break;
				}
				let (sender, receiver) = oneshot::channel();
				queue.pending = Some(sender);
				receiver
			};
			let Some(idle_timeout_ms) = self.idle_timeout_ms.filter(|timeout| *timeout > 0.0) else {
				let _ = receiver.await;
				continue;
			};
			let timeout = pi_host::current().sleep(idle_timeout_ms);
			futures::pin_mut!(timeout);
			if let Either::Right(_) = select(receiver, timeout).await {
				let error = PiError::new(format!(
					"WebSocket idle timeout after {}ms",
					number_to_string(idle_timeout_ms)
				));
				{
					let mut queue = self.queue.borrow_mut();
					queue.failed = Some(error.clone());
					queue.done = true;
					queue.pending = None;
				}
				close_websocket_silently(&*self.socket, 1000, "idle_timeout");
				return Err(error);
			}
		}

		let queue = self.queue.borrow();
		if let Some(failed) = &queue.failed {
			return Err(failed.clone());
		}
		if !queue.saw_completion {
			return Err(PiError::new("WebSocket stream closed before response.completed"));
		}
		Ok(None)
	}
}

impl Drop for WebSocketParser {
	fn drop(&mut self) {
		let Some((listener, abort_listener)) = self.listeners.take() else {
			return;
		};
		self.socket.remove_event_listener(listener);
		if let (Some(signal), Some(abort_listener)) = (&self.signal, abort_listener) {
			signal.remove_listener(abort_listener);
		}
	}
}

fn parse_websocket(
	socket: Rc<dyn WebSocket>,
	signal: Option<AbortSignal>,
	idle_timeout_ms: Option<f64>,
) -> LocalBoxStream<'static, PiResult<Value>> {
	let parser = WebSocketParser {
		socket,
		signal,
		idle_timeout_ms,
		queue: Rc::default(),
		listeners: None,
	};
	futures::stream::unfold(Some(parser), |parser| async move {
		let mut parser = parser?;
		match parser.next_event().await {
			Ok(Some(event)) => Some((Ok(event), Some(parser))),
			Ok(None) => None,
			Err(error) => Some((Err(error), None)),
		}
	})
	.boxed_local()
}

/// `{ input: _input, previous_response_id: _previousResponseId, ...rest }`.
fn request_body_without_input(body: &Value) -> Value {
	let mut rest = body.as_object().cloned().unwrap_or_default();
	rest.shift_remove("input");
	rest.shift_remove("previous_response_id");
	Value::Object(rest)
}

/// `body.input ?? []`.
fn request_input(body: &Value) -> Vec<Value> {
	match body.get("input") {
		Some(Value::Array(items)) => items.clone(),
		_ => Vec::new(),
	}
}

fn request_bodies_match_except_input(a: &Value, b: &Value) -> bool {
	json_stringify(&request_body_without_input(a)) == json_stringify(&request_body_without_input(b))
}

fn get_cached_websocket_input_delta(
	body: &Value,
	continuation: &CachedWebSocketContinuationState,
) -> Option<Vec<Value>> {
	if !request_bodies_match_except_input(body, &continuation.last_request_body) {
		return None;
	}

	let current_input = request_input(body);
	let mut baseline = request_input(&continuation.last_request_body);
	baseline.extend(continuation.last_response_items.iter().cloned());
	if current_input.len() < baseline.len() {
		return None;
	}

	let prefix = &current_input[..baseline.len()];
	if json_stringify(prefix) != json_stringify(&baseline) {
		return None;
	}

	Some(current_input[baseline.len()..].to_vec())
}

fn build_cached_websocket_request_body(entry: &CachedWebSocketConnection, body: &Value) -> Value {
	let mut continuation = entry.continuation.borrow_mut();
	let Some(state) = continuation.as_ref() else {
		return body.clone();
	};

	let delta = get_cached_websocket_input_delta(body, state);
	let (Some(delta), false) = (delta, state.last_response_id.is_empty()) else {
		*continuation = None;
		return body.clone();
	};

	let mut request_body = body.as_object().cloned().unwrap_or_default();
	request_body.insert("previous_response_id".into(), json!(state.last_response_id));
	request_body.insert("input".into(), Value::Array(delta));
	Value::Object(request_body)
}

fn record_websocket_request(cache_session_id: &str, request_body: &Value, reused: bool, use_cached_context: bool) {
	with_websocket_debug_stats(cache_session_id, |stats| {
		stats.requests += 1;
		if reused {
			stats.connections_reused += 1;
		} else {
			stats.connections_created += 1;
		}
		if use_cached_context {
			stats.cached_context_requests += 1;
		}
		if request_body.get("store") == Some(&Value::Bool(true)) {
			stats.store_true_requests += 1;
		}
		let input_items = request_body.get("input").and_then(Value::as_array).map_or(0, Vec::len) as u64;
		stats.last_input_items = input_items;
		match request_body
			.get("previous_response_id")
			.filter(|previous_response_id| truthy(Some(previous_response_id)))
		{
			Some(previous_response_id) => {
				stats.delta_requests += 1;
				stats.last_delta_input_items = Some(input_items);
				stats.last_previous_response_id = Some(js_string(Some(previous_response_id)));
			}
			None => {
				stats.full_context_requests += 1;
				stats.last_delta_input_items = None;
				stats.last_previous_response_id = None;
			}
		}
	});
}

async fn process_websocket_stream(
	url: &str,
	body: &Value,
	headers: &CodexHeaders,
	on_start: Rc<dyn Fn()>,
	request: &CodexRequest<'_>,
) -> PiResult<()> {
	let acquired = acquire_websocket(
		url,
		headers,
		request.cache_session_id,
		request.account_id,
		request.signal(),
		request.websocket_connect_timeout_ms,
	)
	.await?;
	let transport = request.options.and_then(|options| options.transport);
	let use_cached_context = matches!(transport, Some(Transport::WebsocketCached | Transport::Auto));
	// ChatGPT Codex Responses rejects `store: true` ("Store must be set to false").
	// WebSocket continuation still works via connection-scoped previous_response_id state.
	let full_body = body;
	let request_body = match (&acquired.entry, use_cached_context) {
		(Some(entry), true) => build_cached_websocket_request_body(entry, full_body),
		_ => full_body.clone(),
	};
	if let Some(cache_session_id) = request.cache_session_id.filter(|id| !id.is_empty()) {
		record_websocket_request(cache_session_id, &request_body, acquired.reused, use_cached_context);
	}
	let result = send_and_process_websocket_request(
		&acquired,
		full_body,
		&request_body,
		use_cached_context,
		on_start,
		request,
	)
	.await;
	let keep_connection = match &result {
		Ok(keep) => *keep,
		Err(_) => {
			if let Some(entry) = &acquired.entry {
				*entry.continuation.borrow_mut() = None;
			}
			false
		}
	};
	(acquired.release)(keep_connection);
	result.map(|_| ())
}

/// The `try` block of `processWebSocketStream`: whether to keep the connection.
async fn send_and_process_websocket_request(
	acquired: &AcquiredWebSocket,
	full_body: &Value,
	request_body: &Value,
	use_cached_context: bool,
	on_start: Rc<dyn Fn()>,
	request: &CodexRequest<'_>,
) -> PiResult<bool> {
	let (model, output) = (request.model, request.output);
	let mut frame = Map::new();
	frame.insert("type".into(), json!("response.create"));
	for (key, value) in request_body.as_object().into_iter().flatten() {
		frame.insert(key.clone(), value.clone());
	}
	acquired.socket.send(&json_stringify(&Value::Object(frame)))?;
	let started = Cell::new(false);
	let events = map_codex_events(
		parse_websocket(
			acquired.socket.clone(),
			request.signal().cloned(),
			request.http_timeout_ms,
		),
		output.clone(),
		model.clone(),
		request
			.options
			.and_then(|options| options.on_provider_stream_event.clone()),
	)
	.map(move |event| {
		if event.is_ok() && !started.replace(true) {
			on_start();
		}
		event
	});
	process_responses_stream(
		events,
		output,
		request.stream,
		model,
		Some(&responses_stream_options(request)),
	)
	.await?;
	if request.aborted() {
		return Ok(false);
	}
	let response_id = output.borrow().response_id.clone().filter(|id| !id.is_empty());
	if let (true, Some(entry), Some(response_id)) = (use_cached_context, &acquired.entry, response_id) {
		let context = normalize_context(&Context {
			system_prompt: None,
			messages: vec![Message::Assistant(output.borrow().clone())],
			tools: None,
		});
		let response_items = convert_responses_messages(
			model,
			&context,
			&CODEX_TOOL_CALL_PROVIDERS,
			Some(&ConvertResponsesMessagesOptions {
				include_system_prompt: Some(false),
				grammar_tool_input_properties: Some(request.grammar_tool_input_properties.clone()),
				..ConvertResponsesMessagesOptions::default()
			}),
		)?
		.into_iter()
		.filter(|item| {
			!matches!(
				item.get("type").and_then(Value::as_str),
				Some("function_call_output" | "custom_tool_call_output")
			)
		})
		.collect();
		*entry.continuation.borrow_mut() = Some(CachedWebSocketContinuationState {
			last_request_body: full_body.clone(),
			last_response_id: response_id,
			last_response_items: response_items,
		});
	}
	Ok(true)
}

// ============================================================================
// Error Handling
// ============================================================================

struct ErrorResponseInfo {
	message: String,
	friendly_message: Option<String>,
	code: Option<Value>,
	payload: Option<Value>,
}

/// `parseErrorResponse(new Response(errorText, { status, statusText }))`.
fn parse_error_response(raw: &str, status: u16, status_text: &str) -> ErrorResponseInfo {
	let mut info = ErrorResponseInfo {
		message: if !raw.is_empty() {
			raw.to_string()
		} else if !status_text.is_empty() {
			status_text.to_string()
		} else {
			"Request failed".to_string()
		},
		friendly_message: None,
		code: None,
		payload: None,
	};

	let Ok(parsed) = json_parse(raw) else {
		return info;
	};
	let Some(err) = parsed.get("error").filter(|err| truthy(Some(err))).cloned() else {
		return info;
	};
	info.payload = Some(parsed);
	let code = ["code", "type"]
		.iter()
		.filter_map(|field| err.get(*field))
		.find(|value| truthy(Some(value)))
		.cloned()
		.unwrap_or_else(|| json!(""));
	let code_text = js_string(Some(&code)).to_ascii_lowercase();
	info.code = Some(code);
	if ["usage_limit_reached", "usage_not_included", "rate_limit_exceeded"]
		.iter()
		.any(|pattern| code_text.contains(pattern))
		|| status == 429
	{
		let plan = match err.get("plan_type").filter(|plan| truthy(Some(plan))) {
			None => String::new(),
			Some(Value::String(plan)) => format!(" ({} plan)", plan.to_lowercase()),
			// `plan_type.toLowerCase` is not a function: the TS `catch {}` keeps what was set so far.
			Some(_) => return info,
		};
		let when = match err.get("resets_at").filter(|resets_at| truthy(Some(resets_at))) {
			Some(resets_at) => {
				let mins = math_max(
					0.0,
					js_math_round((js_to_number(resets_at) * 1000.0 - pi_host::now_ms()) / 60000.0),
				);
				format!(" Try again in ~{} min.", number_to_string(mins))
			}
			None => String::new(),
		};
		info.friendly_message =
			Some(js_trim(&format!("You have hit your ChatGPT usage limit{plan}.{when}")).to_string());
	}
	if let Some(message) = err.get("message").filter(|message| truthy(Some(message))) {
		info.message = js_string(Some(message));
	} else if let Some(friendly_message) = &info.friendly_message {
		info.message = friendly_message.clone();
	}
	info
}

/// `Math.round(value)`: halves round towards positive infinity.
fn js_math_round(value: f64) -> f64 {
	if !value.is_finite() || value == 0.0 {
		return value;
	}
	// `value + 0.5` can round up in floating point (0.49999999999999994, odd integers above 2^52); `value - floor` is exact.
	let floor = value.floor();
	let rounded = if value - floor >= 0.5 { floor + 1.0 } else { floor };
	if rounded == 0.0 && value < 0.0 { -0.0 } else { rounded }
}

/// `Number(value)` of a JSON value.
fn js_to_number(value: &Value) -> f64 {
	match value {
		Value::Null => 0.0,
		Value::Bool(value) => f64::from(u8::from(*value)),
		Value::Number(number) => number.as_f64().unwrap_or(f64::NAN),
		Value::String(text) => string_to_number(text),
		Value::Array(items) => match items.as_slice() {
			[] => 0.0,
			[item] if !item.is_array() && !item.is_object() => js_to_number(item),
			_ => f64::NAN,
		},
		Value::Object(_) => f64::NAN,
	}
}

// ============================================================================
// Auth & Headers
// ============================================================================

fn extract_account_id(token: &str) -> PiResult<String> {
	let failed = || PiError::new("Failed to extract accountId from token");
	let parts: Vec<&str> = token.split('.').collect();
	if parts.len() != 3 {
		return Err(failed());
	}
	let payload = atob(parts[1]).ok_or_else(failed)?;
	let payload = json_parse(&payload).map_err(|_| failed())?;
	let account_id = payload
		.get(JWT_CLAIM_PATH)
		.and_then(|claim| claim.get("chatgpt_account_id"))
		.filter(|account_id| truthy(Some(account_id)))
		.ok_or_else(failed)?;
	Ok(js_string(Some(account_id)))
}

fn build_base_codex_headers(
	default_headers: &IndexMap<String, String>,
	init_headers: Option<&IndexMap<String, String>>,
	additional_headers: Option<&ProviderHeaders>,
	account_id: &str,
	token: &str,
) -> CodexHeaders {
	// Defaults first so model and caller headers can override them, matching the other providers.
	let mut headers = CodexHeaders::from_record(default_headers);
	for (key, value) in init_headers.into_iter().flatten() {
		headers.set(key, value);
	}
	for (key, value) in additional_headers.into_iter().flatten() {
		match value {
			None => headers.delete(key),
			Some(value) => headers.set(key, value),
		}
	}
	headers.set("Authorization", &format!("Bearer {token}"));
	headers.set("chatgpt-account-id", account_id);
	headers
}

fn build_sse_headers(
	default_headers: &IndexMap<String, String>,
	init_headers: Option<&IndexMap<String, String>>,
	additional_headers: Option<&ProviderHeaders>,
	account_id: &str,
	token: &str,
	session_id: Option<&str>,
) -> CodexHeaders {
	let mut headers = build_base_codex_headers(default_headers, init_headers, additional_headers, account_id, token);
	headers.set("OpenAI-Beta", "responses=experimental");
	headers.set("accept", "text/event-stream");
	headers.set("content-type", "application/json");

	if let Some(session_id) = session_id.filter(|session_id| !session_id.is_empty()) {
		headers.set("session-id", session_id);
		headers.set("x-client-request-id", session_id);
	}

	headers
}

fn build_websocket_headers(
	default_headers: &IndexMap<String, String>,
	init_headers: Option<&IndexMap<String, String>>,
	additional_headers: Option<&ProviderHeaders>,
	account_id: &str,
	token: &str,
	request_id: &str,
) -> CodexHeaders {
	let mut headers = build_base_codex_headers(default_headers, init_headers, additional_headers, account_id, token);
	headers.delete("accept");
	headers.delete("content-type");
	headers.delete("OpenAI-Beta");
	headers.delete("openai-beta");
	headers.set("OpenAI-Beta", OPENAI_BETA_RESPONSES_WEBSOCKETS);
	headers.set("x-client-request-id", request_id);
	headers.set("session-id", request_id);
	headers
}
