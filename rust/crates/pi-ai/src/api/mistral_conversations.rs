//! Port of `packages/ai/src/api/mistral-conversations.ts`.
//! PORT-STATUS: reviewed
//!
//! The TS calls the Mistral HTTP API with `fetch` directly (no SDK), so there is no client file. Differences from the
//! TS:
//! - `MistralOptions` read from `ProviderStreamOptions.extra`: `toolChoice` is any non-null value, `promptMode` and
//!   `reasoningEffort` are read when they are strings.
//! - Stream events are read with the JSON types of the Mistral wire format: a chunk `id`, `finish_reason` or content
//!   text that is not a string is ignored, a `delta.content` that is neither a string nor an array is ignored, a usage
//!   count that is not a number reads as 0, and a choice that is not an object is skipped. The TS throws where Rust is
//!   lenient: a `null` item in a `delta.content` array is skipped, a `delta.tool_calls` that is not an array is
//!   ignored, a tool call `id` that is not a string gets the derived id, a `function.name` that is not a string reads
//!   as `""`, and a content chunk that is not an object is passed through instead of spread. A tool call `index`
//!   that is neither a number nor a string is keyed by its string form.
//! - A lone-surrogate escape in event JSON parses to U+FFFD; the TS removes it.
//! - The `MistralHttpError` is a `PiError` named `MistralHttpError` whose `data` holds `statusCode` and `body`, the
//!   fields `formatMistralError` and `normalizeProviderError` read. A Rust response always has a body, so
//!   `Mistral response has no body` cannot occur.
//! - Messages in a payload that `onPayload` returned are converted when they are objects and passed through otherwise.
//! - The current text or thinking block is always the last content block, so it is tracked by kind, not by object.

use std::cell::RefCell;
use std::collections::HashMap;
use std::ops::{Deref, DerefMut};
use std::rc::Rc;
use std::sync::LazyLock;

use futures::StreamExt;
use futures::future::{Either, select};
use indexmap::IndexMap;
use pi_host::js::{json_stringify, number_to_string};
use pi_host::{AbortSignal, Body, HttpRequest, PiError, PiResult};
use regex::Regex;
use serde_json::{Map, Value, json};

use crate::api::constrained_sampling::{get_json_schema_tool_parameters, resolve_json_schema_strict_sampling};
use crate::api::simple_options::build_base_options;
use crate::api::transform_messages::transform_messages;
use crate::models::{calculate_cost, clamp_thinking_level};
use crate::types::{
	AssistantContent, AssistantMessage, AssistantMessageEvent, CacheRetention, Message, Model, ModelInputModality,
	ModelThinkingLevel, ProviderResponse, ProviderStreamOptions, ProviderStreams, SharedAssistantMessage,
	SimpleStreamOptions, StopReason, StreamOptions, TextContent, ThinkingContent, Tool, ToolCall, ToolChoice,
	TranscriptContext, Usage, UserContent, UserMessageContent,
};
use crate::utils::abort_signals::combine_abort_signals;
use crate::utils::error_body::truncate_error_text;
use crate::utils::event_stream::AssistantMessageEventStream;
use crate::utils::hash::short_hash;
use crate::utils::headers::headers_to_record;
use crate::utils::identity::{ClientIdentityTarget, get_identity_headers};
use crate::utils::json_parse::json_parse;
use crate::utils::sanitize_unicode::sanitize_surrogates;
use crate::utils::streaming_json::ToolCallArgumentsStream;
use crate::utils::text::{get_system_message_text, js_trim, render_system_message_update};
use crate::utils::transcript::{get_current_tools, resolve_transcript};

const MISTRAL_TOOL_CALL_ID_LENGTH: usize = 9;
const MAX_MISTRAL_ERROR_BODY_CHARS: usize = 4000;

/// Provider-specific options for the Mistral API.
#[derive(Clone, Default)]
pub struct MistralOptions {
	pub base: StreamOptions,
	/// `"auto" | "none" | "any" | "required" | { type: "function"; function: { name: string } }`.
	pub tool_choice: Option<Value>,
	/// `"reasoning"`.
	pub prompt_mode: Option<String>,
	/// `"none" | "low" | "medium" | "high" | "max"`.
	pub reasoning_effort: Option<String>,
}

impl Deref for MistralOptions {
	type Target = StreamOptions;

	fn deref(&self) -> &Self::Target {
		&self.base
	}
}

impl DerefMut for MistralOptions {
	fn deref_mut(&mut self) -> &mut Self::Target {
		&mut self.base
	}
}

impl From<ProviderStreamOptions> for MistralOptions {
	/// Reads `toolChoice`, `promptMode` and `reasoningEffort` from the extra keys.
	fn from(options: ProviderStreamOptions) -> Self {
		let extra = options.extra;
		let string = |name: &str| extra.get(name).and_then(Value::as_str).map(str::to_string);
		Self {
			base: options.base,
			tool_choice: extra.get("toolChoice").filter(|value| !value.is_null()).cloned(),
			prompt_mode: string("promptMode"),
			reasoning_effort: string("reasoningEffort"),
		}
	}
}

/// Stream responses from the native Mistral Chat Completions endpoint.
pub fn stream(
	model: &Model,
	context: &TranscriptContext,
	options: Option<MistralOptions>,
) -> AssistantMessageEventStream {
	let stream = AssistantMessageEventStream::new();
	let normalized_context = resolve_transcript(
		context,
		model.mistral_conversations_compat().supports_mid_convo_system_messages,
	);
	let output = create_output(model);
	let options = options.unwrap_or_default();
	let model = model.clone();
	let events = stream.clone();
	pi_host::spawn_local(async move {
		let Err(error) = run_stream(&events, &model, &normalized_context, &options, &output).await else {
			return;
		};
		let reason = {
			let mut message = output.borrow_mut();
			message.stop_reason = if options.signal.as_ref().is_some_and(|signal| signal.aborted()) {
				StopReason::Aborted
			} else {
				StopReason::Error
			};
			message.error_message = Some(format_mistral_error(&error));
			message.stop_reason
		};
		events.push(AssistantMessageEvent::Error {
			reason,
			error: output.clone(),
		});
		events.end(None);
	});
	stream
}

/// The `try` block of `stream`.
async fn run_stream(
	stream: &AssistantMessageEventStream,
	model: &Model,
	context: &TranscriptContext,
	options: &MistralOptions,
	output: &SharedAssistantMessage,
) -> PiResult<()> {
	let Some(api_key) = options.api_key.as_deref().filter(|api_key| !api_key.is_empty()) else {
		return Err(PiError::new(format!("No API key for provider: {}", &*model.provider)));
	};

	let normalize_mistral_tool_call_id = create_mistral_tool_call_id_normalizer();
	let transformed_messages = transform_messages(
		&context.messages,
		model,
		Some(&move |id: &str, _model: &Model, _source: &AssistantMessage| normalize_mistral_tool_call_id(id)),
	);

	let mut payload = build_chat_payload(model, context, &transformed_messages, options)?;
	if let Some(on_payload) = &options.on_payload
		&& let Some(next_payload) = on_payload(payload.clone(), model).await?
	{
		payload = next_payload;
	}
	let mistral_stream = request_mistral_stream(model, &payload, api_key, options).await?;
	stream.push(AssistantMessageEvent::Start {
		partial: output.clone(),
	});
	consume_chat_stream(model, output, stream, mistral_stream, options).await?;

	if options.signal.as_ref().is_some_and(|signal| signal.aborted()) {
		return Err(PiError::new("Request was aborted"));
	}

	let (stop_reason, error_message) = {
		let message = output.borrow();
		(message.stop_reason, message.error_message.clone())
	};
	if stop_reason == StopReason::Pending {
		return Err(PiError::new("Mistral stream ended without a finish reason"));
	}
	if stop_reason == StopReason::Aborted || stop_reason == StopReason::Error {
		return Err(PiError::new(
			error_message
				.filter(|message| !message.is_empty())
				.unwrap_or_else(|| "An unknown error occurred".into()),
		));
	}

	stream.push(AssistantMessageEvent::Done {
		reason: stop_reason,
		message: output.clone(),
	});
	stream.end(None);
	Ok(())
}

/// Maps provider-agnostic `SimpleStreamOptions` to Mistral options.
pub fn stream_simple(
	model: &Model,
	context: &TranscriptContext,
	options: Option<SimpleStreamOptions>,
) -> PiResult<AssistantMessageEventStream> {
	let Some(api_key) = options
		.as_ref()
		.and_then(|options| options.api_key.clone())
		.filter(|api_key| !api_key.is_empty())
	else {
		return Err(PiError::new(format!("No API key for provider: {}", &*model.provider)));
	};

	let base = build_base_options(model, context, options.as_ref(), Some(&api_key));
	let tool_choice = options
		.as_ref()
		.and_then(|options| options.tool_choice)
		.map(|tool_choice| {
			Value::String(
				match tool_choice {
					ToolChoice::Auto => "auto",
					ToolChoice::None => "none",
				}
				.into(),
			)
		});
	let clamped_reasoning = options
		.as_ref()
		.and_then(|options| options.reasoning)
		.map(|reasoning| clamp_thinking_level(model, ModelThinkingLevel::from(reasoning)));
	let reasoning = clamped_reasoning.filter(|reasoning| *reasoning != ModelThinkingLevel::Off);
	// Models with a thinking level map use `reasoning_effort`; other reasoning models use `prompt_mode`.
	let effort_map = model.thinking_level_map.as_ref().filter(|_| model.reasoning);
	let reasoning_effort = effort_map.and_then(|effort_map| match reasoning {
		Some(reasoning) => Some(
			effort_map
				.get(reasoning)
				.flatten()
				.map_or_else(|| "high".to_string(), |effort| effort.to_string()),
		),
		None => effort_map
			.get(ModelThinkingLevel::Off)
			.flatten()
			.map(|effort| effort.to_string()),
	});

	Ok(stream(
		model,
		context,
		Some(MistralOptions {
			base,
			tool_choice,
			prompt_mode: (model.reasoning && effort_map.is_none() && reasoning.is_some())
				.then(|| "reasoning".to_string()),
			reasoning_effort,
		}),
	))
}

/// The mistral-conversations API module as `ProviderStreams`.
pub struct MistralConversationsStreams;

impl ProviderStreams for MistralConversationsStreams {
	fn stream(
		&self,
		model: &Model,
		context: &TranscriptContext,
		options: Option<ProviderStreamOptions>,
	) -> PiResult<AssistantMessageEventStream> {
		Ok(stream(model, context, options.map(MistralOptions::from)))
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

fn create_output(model: &Model) -> SharedAssistantMessage {
	Rc::new(RefCell::new(AssistantMessage {
		content: Vec::new(),
		api: model.api.clone(),
		provider: model.provider.clone(),
		model: model.id.clone(),
		usage: Usage::default(),
		stop_reason: StopReason::Pending,
		timestamp: pi_host::now_ms(),
		..AssistantMessage::default()
	}))
}

fn create_mistral_tool_call_id_normalizer() -> impl Fn(&str) -> String {
	let id_map: RefCell<HashMap<String, String>> = RefCell::default();
	let reverse_map: RefCell<HashMap<String, String>> = RefCell::default();

	move |id: &str| {
		if let Some(existing) = id_map.borrow().get(id).filter(|existing| !existing.is_empty()) {
			return existing.clone();
		}

		let mut attempt = 0;
		loop {
			let candidate = derive_mistral_tool_call_id(id, attempt);
			let owner = reverse_map.borrow().get(&candidate).cloned();
			if owner.as_deref().is_none_or(|owner| owner.is_empty() || owner == id) {
				id_map.borrow_mut().insert(id.to_string(), candidate.clone());
				reverse_map.borrow_mut().insert(candidate.clone(), id.to_string());
				return candidate;
			}
			attempt += 1;
		}
	}
}

static NON_ALPHANUMERIC: LazyLock<Regex> = LazyLock::new(|| Regex::new("[^a-zA-Z0-9]").expect("valid regex"));

fn derive_mistral_tool_call_id(id: &str, attempt: u32) -> String {
	let normalized = NON_ALPHANUMERIC.replace_all(id, "");
	if attempt == 0 && normalized.len() == MISTRAL_TOOL_CALL_ID_LENGTH {
		return normalized.into_owned();
	}
	let seed_base = if normalized.is_empty() { id } else { &normalized };
	let seed = if attempt == 0 {
		seed_base.to_string()
	} else {
		format!("{seed_base}:{attempt}")
	};
	NON_ALPHANUMERIC
		.replace_all(&short_hash(&seed), "")
		.chars()
		.take(MISTRAL_TOOL_CALL_ID_LENGTH)
		.collect()
}

fn format_mistral_error(error: &PiError) -> String {
	let data = error.data.as_ref();
	let status_code = data.and_then(|data| data.get("statusCode")).and_then(Value::as_f64);
	let body_text = data
		.and_then(|data| data.get("body"))
		.and_then(Value::as_str)
		.map(js_trim)
		.filter(|body| !body.is_empty());
	match (status_code, body_text) {
		(Some(status_code), Some(body_text)) => format!(
			"Mistral API error ({}): {}",
			number_to_string(status_code),
			truncate_error_text(body_text, MAX_MISTRAL_ERROR_BODY_CHARS)
		),
		(Some(status_code), None) => {
			format!(
				"Mistral API error ({}): {}",
				number_to_string(status_code),
				error.message
			)
		}
		_ => error.message.clone(),
	}
}

async fn request_mistral_stream(
	model: &Model,
	payload: &Value,
	api_key: &str,
	options: &MistralOptions,
) -> PiResult<MistralEvents> {
	let mut base_url = url::Url::parse(&model.base_url).map_err(|_| PiError::type_error("Invalid URL"))?;
	let path = format!("{}/", base_url.path().trim_end_matches('/'));
	base_url.set_path(&path);
	let url = base_url
		.join("v1/chat/completions")
		.map_err(|_| PiError::type_error("Invalid URL"))?;
	let headers = build_mistral_headers(model, api_key, options)?;
	// The timeout covers only the wait for response headers. Long streams (e.g. extended thinking)
	// must not be cut off by a fixed deadline; body stalls are left to the HTTP client idle timeout.
	let timeout_ms = options.timeout_ms.unwrap_or(60_000.0);
	let header_timeout_signal = AbortSignal::timeout(timeout_ms);
	// The body is built before the signals are combined, so a conversion error leaves no listener behind.
	let body = json_stringify(&to_mistral_wire_payload(payload)?);
	let combined_signal = combine_abort_signals(&[options.signal.as_ref(), Some(&header_timeout_signal)]);
	let request = HttpRequest {
		method: "POST".into(),
		url: url.to_string(),
		headers,
		body: Some(body.into()),
		signal: combined_signal.signal.clone(),
		..Default::default()
	};
	let response = pi_host::fetch(request, options.fetch.as_ref()).await;
	combined_signal.cleanup();
	let response = match response {
		Ok(response) => response,
		Err(error) => {
			if header_timeout_signal.aborted() && !options.signal.as_ref().is_some_and(|signal| signal.aborted()) {
				return Err(PiError::new(format!(
					"Mistral response headers timed out after {}ms",
					number_to_string(timeout_ms)
				)));
			}
			return Err(error);
		}
	};

	if let Some(on_response) = &options.on_response {
		on_response(
			ProviderResponse {
				status: response.status,
				headers: headers_to_record(&response.headers),
			},
			model,
		)
		.await?;
	}

	if !response.ok() {
		let status = response.status;
		let status_text = response.status_text.clone();
		let body = response.text().await?;
		return Err(mistral_http_error(status, body, &status_text));
	}

	Ok(MistralEvents {
		body: response.body,
		signal: options.signal.clone(),
		decoder: Utf8Decoder::default(),
		buffer: String::new(),
		done: false,
		finished: false,
	})
}

/// `new MistralHttpError(statusCode, body, statusText)`.
fn mistral_http_error(status_code: u16, body: String, status_text: &str) -> PiError {
	let message = if status_text.is_empty() {
		format!("Request failed with status {status_code}")
	} else {
		status_text.to_string()
	};
	PiError::named("MistralHttpError", message).with_data(json!({ "statusCode": status_code, "body": body }))
}

/// `new Headers({...})` with the overrides applied: lowercase names, listed in sorted order like `Headers` iteration.
fn build_mistral_headers(model: &Model, api_key: &str, options: &MistralOptions) -> PiResult<IndexMap<String, String>> {
	let mut headers: IndexMap<String, String> = IndexMap::new();
	let identity_headers = get_identity_headers(
		ClientIdentityTarget {
			provider: &model.provider,
			base_url: &model.base_url,
		},
		options.identity.as_ref(),
	)?;
	for (name, value) in identity_headers {
		headers.insert(name.to_lowercase(), value);
	}
	headers.insert("accept".into(), "text/event-stream".into());
	headers.insert("authorization".into(), format!("Bearer {api_key}"));
	headers.insert("content-type".into(), "application/json".into());
	for (name, value) in model.headers.iter().flatten() {
		headers.insert(name.to_lowercase(), value.clone());
	}
	for (name, value) in options.headers.iter().flatten() {
		match value {
			Some(value) => headers.insert(name.to_lowercase(), value.clone()),
			None => headers.shift_remove(&name.to_lowercase()),
		};
	}

	let has_explicit_affinity = model
		.headers
		.iter()
		.flatten()
		.any(|(name, _)| name.to_lowercase() == "x-affinity")
		|| options
			.headers
			.iter()
			.flatten()
			.any(|(name, _)| name.to_lowercase() == "x-affinity");
	if let Some(session_id) = prompt_cache_session_id(options)
		&& !has_explicit_affinity
	{
		headers.insert("x-affinity".into(), session_id.to_string());
	}

	headers.sort_keys();
	Ok(headers)
}

const PAYLOAD_PROPERTIES: [(&str, &str); 12] = [
	("topP", "top_p"),
	("maxTokens", "max_tokens"),
	("randomSeed", "random_seed"),
	("responseFormat", "response_format"),
	("toolChoice", "tool_choice"),
	("presencePenalty", "presence_penalty"),
	("frequencyPenalty", "frequency_penalty"),
	("parallelToolCalls", "parallel_tool_calls"),
	("reasoningEffort", "reasoning_effort"),
	("promptMode", "prompt_mode"),
	("promptCacheKey", "prompt_cache_key"),
	("safePrompt", "safe_prompt"),
];

fn to_mistral_wire_payload(payload: &Value) -> PiResult<Value> {
	let Some(payload) = payload.as_object() else {
		return Err(PiError::type_error(if payload.is_null() {
			"Cannot read properties of null (reading 'messages')"
		} else {
			"Cannot read properties of undefined (reading 'map')"
		}));
	};
	let mut wire_payload = payload.clone();
	for (source, target) in PAYLOAD_PROPERTIES {
		remap_mistral_property(&mut wire_payload, source, target);
	}
	let messages = match payload.get("messages") {
		Some(Value::Array(messages)) => messages,
		None => {
			return Err(PiError::type_error(
				"Cannot read properties of undefined (reading 'map')",
			));
		}
		Some(Value::Null) => return Err(PiError::type_error("Cannot read properties of null (reading 'map')")),
		Some(_) => return Err(PiError::type_error("payload.messages.map is not a function")),
	};
	wire_payload.insert(
		"messages".into(),
		Value::Array(messages.iter().map(to_mistral_wire_message).collect()),
	);

	if let Some(Value::Object(response_format)) = wire_payload.get("response_format") {
		let mut wire_response_format = response_format.clone();
		remap_mistral_property(&mut wire_response_format, "jsonSchema", "json_schema");
		if let Some(Value::Object(json_schema)) = wire_response_format.get("json_schema") {
			let mut wire_json_schema = json_schema.clone();
			remap_mistral_property(&mut wire_json_schema, "schemaDefinition", "schema");
			wire_response_format.insert("json_schema".into(), Value::Object(wire_json_schema));
		}
		wire_payload.insert("response_format".into(), Value::Object(wire_response_format));
	}

	Ok(Value::Object(wire_payload))
}

fn to_mistral_wire_message(message: &Value) -> Value {
	let Some(message) = message.as_object() else {
		return message.clone();
	};
	let mut wire_message = message.clone();
	remap_mistral_property(&mut wire_message, "toolCalls", "tool_calls");
	remap_mistral_property(&mut wire_message, "toolCallId", "tool_call_id");
	if let Some(Value::Array(content)) = message.get("content") {
		wire_message.insert(
			"content".into(),
			Value::Array(content.iter().map(to_mistral_wire_content_chunk).collect()),
		);
	}
	Value::Object(wire_message)
}

const CONTENT_CHUNK_PROPERTIES: [(&str, &str); 6] = [
	("imageUrl", "image_url"),
	("documentUrl", "document_url"),
	("documentName", "document_name"),
	("fileId", "file_id"),
	("referenceIds", "reference_ids"),
	("inputAudio", "input_audio"),
];

fn to_mistral_wire_content_chunk(chunk: &Value) -> Value {
	let Some(chunk) = chunk.as_object() else {
		return chunk.clone();
	};
	let mut wire_chunk = chunk.clone();
	for (source, target) in CONTENT_CHUNK_PROPERTIES {
		remap_mistral_property(&mut wire_chunk, source, target);
	}
	Value::Object(wire_chunk)
}

fn remap_mistral_property(record: &mut Map<String, Value>, source: &str, target: &str) {
	let Some(value) = record.get(source).cloned() else {
		return;
	};
	record.insert(target.into(), value);
	record.shift_remove(source);
}

/// `readMistralEvents`: the parsed SSE events of a response body.
struct MistralEvents {
	body: Body,
	signal: Option<AbortSignal>,
	decoder: Utf8Decoder,
	buffer: String,
	done: bool,
	finished: bool,
}

/// What `parseMistralEvent` found in one raw event.
enum ParsedMistralEvent {
	Empty,
	Done,
	Event(Value),
}

static MISTRAL_EVENT_BOUNDARY: LazyLock<Regex> =
	LazyLock::new(|| Regex::new(r"\r\n\r\n|\r\n\r|\r\n\n|\r\r\n|\n\r\n|\r\r|\n\r|\n\n").expect("valid regex"));

impl MistralEvents {
	/// The next event, or `None` after `[DONE]` or the end of the body.
	async fn next(&mut self) -> PiResult<Option<Value>> {
		loop {
			if self.finished {
				return Ok(None);
			}
			if let Some(boundary) = MISTRAL_EVENT_BOUNDARY.find(&self.buffer) {
				let (index, end) = (boundary.start(), boundary.end());
				let event = parse_mistral_event(&self.buffer[..index])?;
				self.buffer.drain(..end);
				match event {
					ParsedMistralEvent::Done => {
						self.finished = true;
						return Ok(None);
					}
					ParsedMistralEvent::Event(event) => return Ok(Some(event)),
					ParsedMistralEvent::Empty => continue,
				}
			}
			if self.done {
				self.finished = true;
				if js_trim(&self.buffer).is_empty() {
					return Ok(None);
				}
				return Ok(match parse_mistral_event(&self.buffer)? {
					ParsedMistralEvent::Event(event) => Some(event),
					ParsedMistralEvent::Done | ParsedMistralEvent::Empty => None,
				});
			}
			self.read().await?;
		}
	}

	/// One `reader.read()`. An abort cancels the pending read and fails with the signal's reason.
	async fn read(&mut self) -> PiResult<()> {
		if let Some(reason) = self.signal.as_ref().and_then(AbortSignal::reason) {
			return Err(reason);
		}
		let chunk = match &self.signal {
			Some(signal) => match select(self.body.next(), Box::pin(signal.aborted_future())).await {
				Either::Left((chunk, _)) => chunk,
				Either::Right((reason, _)) => return Err(reason),
			},
			None => self.body.next().await,
		};
		if let Some(reason) = self.signal.as_ref().and_then(AbortSignal::reason) {
			return Err(reason);
		}
		match chunk {
			Some(chunk) => {
				let decoded = self.decoder.decode(&chunk?);
				self.buffer.push_str(&decoded);
			}
			None => {
				let decoded = self.decoder.flush();
				self.buffer.push_str(&decoded);
				self.done = true;
			}
		}
		Ok(())
	}
}

static LINE_BREAK: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\r\n|\r|\n").expect("valid regex"));

fn parse_mistral_event(raw: &str) -> PiResult<ParsedMistralEvent> {
	let data_lines: Vec<&str> = LINE_BREAK
		.split(raw)
		.filter_map(|line| line.strip_prefix("data:"))
		.map(js_trim_start)
		.collect();
	let data = data_lines.join("\n");
	let data = js_trim(&data);
	if data.is_empty() {
		return Ok(ParsedMistralEvent::Empty);
	}
	if data == "[DONE]" {
		return Ok(ParsedMistralEvent::Done);
	}

	let parsed = json_parse(data)?;
	if !parsed.get("choices").is_some_and(Value::is_array) {
		return Err(PiError::new("Invalid Mistral streaming event"));
	}
	Ok(ParsedMistralEvent::Event(parsed))
}

/// `text.trimStart()`: removes JavaScript white space and line terminators.
fn js_trim_start(text: &str) -> &str {
	text.trim_start_matches(|ch: char| (ch.is_whitespace() && ch != '\u{85}') || ch == '\u{feff}')
}

/// `new TextDecoder()`: lossy UTF-8 that drops a leading BOM and keeps a split sequence for the next chunk.
#[derive(Default)]
struct Utf8Decoder {
	pending: Vec<u8>,
	bom_checked: bool,
}

impl Utf8Decoder {
	/// `decoder.decode(chunk, { stream: true })`.
	fn decode(&mut self, chunk: &[u8]) -> String {
		self.pending.extend_from_slice(chunk);
		if !self.bom_checked {
			if self.pending.len() < 3 && [0xEF, 0xBB, 0xBF].starts_with(&self.pending) {
				return String::new();
			}
			self.bom_checked = true;
			if self.pending.starts_with(&[0xEF, 0xBB, 0xBF]) {
				self.pending.drain(..3);
			}
		}
		let mut out = String::new();
		let mut start = 0;
		loop {
			match std::str::from_utf8(&self.pending[start..]) {
				Ok(valid) => {
					out.push_str(valid);
					start = self.pending.len();
					break;
				}
				Err(error) => {
					let valid_up_to = start + error.valid_up_to();
					out.push_str(
						std::str::from_utf8(&self.pending[start..valid_up_to]).expect("valid_up_to marks valid UTF-8"),
					);
					match error.error_len() {
						Some(length) => {
							out.push('\u{FFFD}');
							start = valid_up_to + length;
						}
						None => {
							start = valid_up_to;
							break;
						}
					}
				}
			}
		}
		self.pending.drain(..start);
		out
	}

	/// `decoder.decode()`: the end of the input, where an unfinished sequence becomes U+FFFD.
	fn flush(&mut self) -> String {
		self.bom_checked = true;
		let out = String::from_utf8_lossy(&self.pending).into_owned();
		self.pending.clear();
		out
	}
}

fn build_chat_payload(
	model: &Model,
	context: &TranscriptContext,
	messages: &[Message],
	options: &MistralOptions,
) -> PiResult<Value> {
	let mut payload = Map::new();
	payload.insert("model".into(), Value::String(model.id.clone()));
	payload.insert("stream".into(), Value::Bool(true));
	payload.insert(
		"messages".into(),
		Value::Array(to_chat_messages(
			messages,
			model.input.contains(&ModelInputModality::Image),
		)),
	);

	let current_tools = get_current_tools(&context.messages);
	if !current_tools.is_empty() {
		payload.insert("tools".into(), Value::Array(to_function_tools(&current_tools)?));
	}
	if let Some(temperature) = options.temperature {
		payload.insert("temperature".into(), json!(temperature));
	}
	if let Some(max_tokens) = options.max_tokens {
		payload.insert("maxTokens".into(), json!(max_tokens));
	}
	if let Some(tool_choice) = options.tool_choice.as_ref().filter(|choice| js_truthy(choice)) {
		payload.insert("toolChoice".into(), map_tool_choice(tool_choice)?);
	}
	if let Some(prompt_mode) = options.prompt_mode.as_ref().filter(|mode| !mode.is_empty()) {
		payload.insert("promptMode".into(), Value::String(prompt_mode.clone()));
	}
	if let Some(reasoning_effort) = options.reasoning_effort.as_ref().filter(|effort| !effort.is_empty()) {
		payload.insert("reasoningEffort".into(), Value::String(reasoning_effort.clone()));
	}
	if let Some(session_id) = prompt_cache_session_id(options) {
		payload.insert("promptCacheKey".into(), Value::String(session_id.to_string()));
	}

	Ok(Value::Object(payload))
}

/// `shouldUsePromptCaching(options)`: the session id when it is used for prompt caching.
fn prompt_cache_session_id(options: &MistralOptions) -> Option<&str> {
	if options.cache_retention == Some(CacheRetention::None) {
		return None;
	}
	options
		.session_id
		.as_deref()
		.filter(|session_id| !session_id.is_empty())
}

fn js_truthy(value: &Value) -> bool {
	match value {
		Value::Null => false,
		Value::Bool(value) => *value,
		Value::Number(number) => number.as_f64().is_some_and(|number| number != 0.0 && !number.is_nan()),
		Value::String(text) => !text.is_empty(),
		Value::Array(_) | Value::Object(_) => true,
	}
}

/// A JSON number that reads as 0 when it is missing, not a number, or 0 (`value || 0`).
fn count(value: Option<&Value>) -> f64 {
	value
		.and_then(Value::as_f64)
		.filter(|count| *count != 0.0 && !count.is_nan())
		.unwrap_or(0.0)
}

fn get_mistral_cached_prompt_tokens(usage: &Map<String, Value>, prompt_tokens: f64) -> f64 {
	let nested = |outer: &str, inner: &str| usage.get(outer).and_then(|outer| outer.get(inner));
	let raw_cached_tokens = [
		nested("promptTokensDetails", "cachedTokens"),
		nested("prompt_tokens_details", "cached_tokens"),
		nested("promptTokenDetails", "cachedTokens"),
		nested("prompt_token_details", "cached_tokens"),
		usage.get("numCachedTokens"),
		usage.get("num_cached_tokens"),
	]
	.into_iter()
	.flatten()
	.find(|value| !value.is_null());
	let cached_tokens = raw_cached_tokens
		.and_then(Value::as_f64)
		.filter(|tokens| tokens.is_finite())
		.unwrap_or(0.0);
	prompt_tokens.min(cached_tokens.max(0.0))
}

/// `toolCallsByKey`'s key: `toolCall.index ?? callId`. A JS `Map` tells the number `0` from the string `"0"`, and a
/// string index shares keys with call ids.
#[derive(Clone, PartialEq, Eq, Hash)]
enum ToolCallKey {
	Number(String),
	String(String),
}

/// Stream state of one Mistral tool call, kept off the content blocks so it never reaches partials.
struct OpenToolCall {
	content_index: usize,
	arguments: ToolCallArgumentsStream,
}

/// `currentBlock`'s type. The block itself is the last element of `output.content`.
#[derive(Clone, Copy, PartialEq, Eq)]
enum CurrentBlock {
	Text,
	Thinking,
}

/// The block bookkeeping of one streamed response.
struct ChatStreamBlocks<'a> {
	stream: &'a AssistantMessageEventStream,
	output: &'a SharedAssistantMessage,
	current_block: Option<CurrentBlock>,
}

impl ChatStreamBlocks<'_> {
	fn block_index(&self) -> usize {
		self.output.borrow().content.len() - 1
	}

	/// `finishCurrentBlock(currentBlock)` and `currentBlock = null`.
	fn finish_current_block(&mut self) {
		let Some(current_block) = self.current_block.take() else {
			return;
		};
		let content_index = self.block_index();
		let content = match &self.output.borrow().content[content_index] {
			AssistantContent::Text(block) => block.text.clone(),
			AssistantContent::Thinking(block) => block.thinking.clone(),
			AssistantContent::ToolCall(_) => unreachable!("the current block is the last content block"),
		};
		let partial = self.output.clone();
		self.stream.push(match current_block {
			CurrentBlock::Text => AssistantMessageEvent::TextEnd {
				content_index,
				content,
				partial,
			},
			CurrentBlock::Thinking => AssistantMessageEvent::ThinkingEnd {
				content_index,
				content,
				partial,
			},
		});
	}

	/// Appends a non-empty delta to the current block of `kind`, opening one first when needed.
	fn push_delta(&mut self, kind: CurrentBlock, delta: &str) {
		if self.current_block != Some(kind) {
			self.finish_current_block();
			let block = match kind {
				CurrentBlock::Text => AssistantContent::Text(TextContent::default()),
				CurrentBlock::Thinking => AssistantContent::Thinking(ThinkingContent::default()),
			};
			self.output.borrow_mut().content.push(block);
			self.current_block = Some(kind);
			let content_index = self.block_index();
			let partial = self.output.clone();
			self.stream.push(match kind {
				CurrentBlock::Text => AssistantMessageEvent::TextStart { content_index, partial },
				CurrentBlock::Thinking => AssistantMessageEvent::ThinkingStart { content_index, partial },
			});
		}
		let content_index = self.block_index();
		match self.output.borrow_mut().content.last_mut() {
			Some(AssistantContent::Text(block)) => block.text.push_str(delta),
			Some(AssistantContent::Thinking(block)) => block.thinking.push_str(delta),
			_ => unreachable!("the current block is the last content block"),
		}
		let delta = delta.to_string();
		let partial = self.output.clone();
		self.stream.push(match kind {
			CurrentBlock::Text => AssistantMessageEvent::TextDelta {
				content_index,
				delta,
				partial,
			},
			CurrentBlock::Thinking => AssistantMessageEvent::ThinkingDelta {
				content_index,
				delta,
				partial,
			},
		});
	}
}

async fn consume_chat_stream(
	model: &Model,
	output: &SharedAssistantMessage,
	stream: &AssistantMessageEventStream,
	mut mistral_stream: MistralEvents,
	options: &MistralOptions,
) -> PiResult<()> {
	let mut blocks = ChatStreamBlocks {
		stream,
		output,
		current_block: None,
	};
	let mut tool_calls_by_key: IndexMap<ToolCallKey, OpenToolCall> = IndexMap::new();

	while let Some(chunk) = mistral_stream.next().await? {
		if let Some(on_provider_stream_event) = &options.on_provider_stream_event {
			on_provider_stream_event(&chunk, model).await?;
		}
		{
			// Mistral's streamed CompletionChunk carries an id field. Keep the first non-empty one,
			// mirroring how OpenAI-style streaming exposes a stable response identifier per stream.
			let mut message = output.borrow_mut();
			if message.response_id.as_deref().is_none_or(str::is_empty) {
				message.response_id = chunk.get("id").and_then(Value::as_str).map(str::to_string);
			}

			if let Some(usage) = chunk.get("usage").and_then(Value::as_object) {
				let prompt_tokens = count(usage.get("prompt_tokens"));
				let cached_prompt_tokens = get_mistral_cached_prompt_tokens(usage, prompt_tokens);

				let message_usage = &mut message.usage;
				message_usage.input = (prompt_tokens - cached_prompt_tokens).max(0.0);
				message_usage.output = count(usage.get("completion_tokens"));
				message_usage.cache_read = cached_prompt_tokens;
				message_usage.cache_write = 0.0;
				message_usage.total_tokens = match count(usage.get("total_tokens")) {
					0.0 => {
						message_usage.input
							+ message_usage.output
							+ message_usage.cache_read
							+ message_usage.cache_write
					}
					total_tokens => total_tokens,
				};
				calculate_cost(model, message_usage);
			}
		}

		let Some(choice) = chunk
			.get("choices")
			.and_then(Value::as_array)
			.and_then(|choices| choices.first())
			.and_then(Value::as_object)
		else {
			continue;
		};

		if let Some(finish_reason) = choice
			.get("finish_reason")
			.and_then(Value::as_str)
			.filter(|reason| !reason.is_empty())
		{
			let (stop_reason, error_message) = map_chat_stop_reason(finish_reason);
			let mut message = output.borrow_mut();
			message.raw_stop_reason = Some(finish_reason.to_string());
			message.stop_reason = stop_reason;
			if let Some(error_message) = error_message {
				message.error_message = Some(error_message);
			}
		}

		let delta = match choice.get("delta") {
			Some(Value::Object(delta)) => delta,
			Some(Value::Null) => {
				return Err(PiError::type_error(
					"Cannot read properties of null (reading 'content')",
				));
			}
			None => {
				return Err(PiError::type_error(
					"Cannot read properties of undefined (reading 'content')",
				));
			}
			Some(_) => continue,
		};
		let content_items: Vec<&Value> = match delta.get("content") {
			Some(content @ Value::String(_)) => vec![content],
			Some(Value::Array(content)) => content.iter().collect(),
			_ => Vec::new(),
		};
		for item in content_items {
			if let Value::String(item) = item {
				let text_delta = sanitize_surrogates(item);
				// GLM models on Mistral send empty content deltas around thinking and tool calls.
				// Opening a block for them splits thinking into multiple blocks, which Mistral rejects on replay.
				if !text_delta.is_empty() {
					blocks.push_delta(CurrentBlock::Text, text_delta);
				}
				continue;
			}

			match item.get("type").and_then(Value::as_str) {
				Some("thinking") => {
					let delta_text: String = item
						.get("thinking")
						.and_then(Value::as_array)
						.into_iter()
						.flatten()
						.filter_map(|part| part.get("text").and_then(Value::as_str))
						.collect();
					let thinking_delta = sanitize_surrogates(&delta_text);
					if !thinking_delta.is_empty() {
						blocks.push_delta(CurrentBlock::Thinking, thinking_delta);
					}
				}
				Some("text") => {
					let text_delta = sanitize_surrogates(item.get("text").and_then(Value::as_str).unwrap_or_default());
					if !text_delta.is_empty() {
						blocks.push_delta(CurrentBlock::Text, text_delta);
					}
				}
				_ => {}
			}
		}

		let tool_calls = delta
			.get("tool_calls")
			.and_then(Value::as_array)
			.map(Vec::as_slice)
			.unwrap_or_default();
		for tool_call in tool_calls {
			blocks.finish_current_block();
			let index = tool_call.get("index").filter(|index| !index.is_null());
			let call_id = match tool_call.get("id").and_then(Value::as_str) {
				Some(id) if !id.is_empty() && id != "null" => id.to_string(),
				_ => derive_mistral_tool_call_id(
					&format!("toolcall:{}", index.map_or_else(|| "0".to_string(), js_string)),
					0,
				),
			};
			let key = match index {
				Some(Value::Number(number)) => {
					ToolCallKey::Number(number_to_string(number.as_f64().unwrap_or(f64::NAN)))
				}
				Some(index) => ToolCallKey::String(js_string(index)),
				None => ToolCallKey::String(call_id.clone()),
			};
			let function = tool_call.get("function");
			let open = match tool_calls_by_key.entry(key) {
				indexmap::map::Entry::Occupied(entry) => entry.into_mut(),
				indexmap::map::Entry::Vacant(entry) => {
					let name = read_property(function, "name")?
						.and_then(Value::as_str)
						.unwrap_or_default()
						.to_string();
					let content_index = {
						let mut message = output.borrow_mut();
						message.content.push(AssistantContent::ToolCall(ToolCall {
							id: call_id,
							name,
							..ToolCall::default()
						}));
						message.content.len() - 1
					};
					stream.push(AssistantMessageEvent::ToolcallStart {
						content_index,
						partial: output.clone(),
					});
					entry.insert(OpenToolCall {
						content_index,
						arguments: ToolCallArgumentsStream::new(),
					})
				}
			};

			let args_delta = match read_property(function, "arguments")? {
				Some(Value::String(arguments)) => arguments.clone(),
				Some(arguments) if js_truthy(arguments) => json_stringify(arguments),
				_ => "{}".to_string(),
			};
			match &mut output.borrow_mut().content[open.content_index] {
				AssistantContent::ToolCall(block) => open.arguments.append(&args_delta, &mut block.arguments),
				_ => unreachable!("an open tool call indexes a tool call block"),
			}
			stream.push(AssistantMessageEvent::ToolcallDelta {
				content_index: open.content_index,
				delta: args_delta,
				partial: output.clone(),
			});
		}
	}

	blocks.finish_current_block();
	for open in tool_calls_by_key.values() {
		let finished = open.arguments.finish();
		let tool_call = {
			let mut message = output.borrow_mut();
			let AssistantContent::ToolCall(block) = &mut message.content[open.content_index] else {
				unreachable!("open tool calls index tool call blocks");
			};
			block.arguments = finished.arguments;
			block.arguments_error = finished.arguments_error;
			block.clone()
		};
		stream.push(AssistantMessageEvent::ToolcallEnd {
			content_index: open.content_index,
			tool_call,
			partial: output.clone(),
		});
	}
	Ok(())
}

/// `value[key]` for a JSON value: the TypeError V8 reports on `undefined` and `null`, and `undefined` for a key a
/// non-object value lacks.
fn read_property<'a>(value: Option<&'a Value>, key: &str) -> PiResult<Option<&'a Value>> {
	match value {
		None => Err(PiError::type_error(format!(
			"Cannot read properties of undefined (reading '{key}')"
		))),
		Some(Value::Null) => Err(PiError::type_error(format!(
			"Cannot read properties of null (reading '{key}')"
		))),
		Some(value) => Ok(value.get(key)),
	}
}

/// `String(value)` of a JSON scalar.
fn js_string(value: &Value) -> String {
	match value {
		Value::String(text) => text.clone(),
		Value::Number(number) => number_to_string(number.as_f64().unwrap_or(f64::NAN)),
		other => json_stringify(other),
	}
}

fn to_function_tools(tools: &[Tool]) -> PiResult<Vec<Value>> {
	tools
		.iter()
		.map(|tool| {
			let strict = resolve_json_schema_strict_sampling(tool, true, None)?;
			Ok(json!({
				"type": "function",
				"function": {
					"name": tool.name,
					"description": tool.description,
					"parameters": get_json_schema_tool_parameters(tool, strict)?,
					"strict": strict.unwrap_or(false),
				},
			}))
		})
		.collect()
}

fn image_url(mime_type: &str, data: &str) -> String {
	format!("data:{mime_type};base64,{data}")
}

fn to_chat_messages(messages: &[Message], supports_images: bool) -> Vec<Value> {
	let mut result = Vec::new();

	for (index, msg) in messages.iter().enumerate() {
		match msg {
			Message::System(msg) => {
				let text = if index == 0 {
					get_system_message_text(msg)
				} else {
					render_system_message_update(msg)
				};
				if !text.is_empty() {
					result.push(json!({ "role": "system", "content": sanitize_surrogates(&text) }));
				}
			}
			Message::User(msg) => {
				let blocks = match &msg.content {
					UserMessageContent::Text(text) => {
						result.push(json!({ "role": "user", "content": sanitize_surrogates(text) }));
						continue;
					}
					UserMessageContent::Blocks(blocks) => blocks,
				};
				let had_images = blocks.iter().any(|item| matches!(item, UserContent::Image(_)));
				let content: Vec<Value> = blocks
					.iter()
					.filter_map(|item| match item {
						UserContent::Text(item) => {
							Some(json!({ "type": "text", "text": sanitize_surrogates(&item.text) }))
						}
						UserContent::Image(item) if supports_images => Some(json!({
							"type": "image_url",
							"imageUrl": image_url(&item.mime_type, &item.data),
						})),
						UserContent::Image(_) => None,
					})
					.collect();
				if !content.is_empty() {
					result.push(json!({ "role": "user", "content": content }));
					continue;
				}
				if had_images && !supports_images {
					result.push(json!({ "role": "user", "content": "(image omitted: model does not support images)" }));
				}
			}
			Message::Assistant(msg) => {
				let mut content_parts = Vec::new();
				let mut tool_calls = Vec::new();

				for block in &msg.content {
					match block {
						AssistantContent::Text(block) => {
							if !js_trim(&block.text).is_empty() {
								content_parts.push(json!({ "type": "text", "text": sanitize_surrogates(&block.text) }));
							}
						}
						AssistantContent::Thinking(block) => {
							if !js_trim(&block.thinking).is_empty() {
								content_parts.push(json!({
									"type": "thinking",
									"thinking": [{ "type": "text", "text": sanitize_surrogates(&block.thinking) }],
								}));
							}
						}
						AssistantContent::ToolCall(block) => tool_calls.push(json!({
							"id": block.id,
							"type": "function",
							"function": {
								"name": block.name,
								"arguments": json_stringify(&Value::Object(block.arguments.clone())),
							},
							"index": 0.0,
						})),
					}
				}

				let mut assistant_message = Map::new();
				assistant_message.insert("role".into(), Value::String("assistant".into()));
				assistant_message.insert("prefix".into(), Value::Bool(false));
				let has_content = !content_parts.is_empty() || !tool_calls.is_empty();
				if !content_parts.is_empty() {
					assistant_message.insert("content".into(), Value::Array(content_parts));
				}
				if !tool_calls.is_empty() {
					assistant_message.insert("toolCalls".into(), Value::Array(tool_calls));
				}
				if has_content {
					result.push(Value::Object(assistant_message));
				}
			}
			Message::ToolResult(msg) => {
				let text_result = msg
					.content
					.iter()
					.filter_map(|part| match part {
						UserContent::Text(part) => Some(sanitize_surrogates(&part.text)),
						UserContent::Image(_) => None,
					})
					.collect::<Vec<_>>()
					.join("\n");
				let has_images = msg.content.iter().any(|part| matches!(part, UserContent::Image(_)));
				let tool_text = build_tool_result_text(&text_result, has_images, supports_images, msg.is_error);
				let mut tool_content = vec![json!({ "type": "text", "text": tool_text })];
				if supports_images {
					for part in &msg.content {
						if let UserContent::Image(part) = part {
							tool_content.push(json!({
								"type": "image_url",
								"imageUrl": image_url(&part.mime_type, &part.data),
							}));
						}
					}
				}
				result.push(json!({
					"role": "tool",
					"toolCallId": msg.tool_call_id,
					"name": msg.tool_name,
					"content": tool_content,
				}));
			}
		}
	}

	result
}

fn build_tool_result_text(text: &str, has_images: bool, supports_images: bool, is_error: bool) -> String {
	let trimmed = js_trim(text);
	let error_prefix = if is_error { "[tool error] " } else { "" };

	if !trimmed.is_empty() {
		let image_suffix = if has_images && !supports_images {
			"\n[tool image omitted: model does not support images]"
		} else {
			""
		};
		return format!("{error_prefix}{trimmed}{image_suffix}");
	}

	if has_images {
		if supports_images {
			return if is_error {
				"[tool error] (see attached image)"
			} else {
				"(see attached image)"
			}
			.into();
		}
		return if is_error {
			"[tool error] (image omitted: model does not support images)"
		} else {
			"(image omitted: model does not support images)"
		}
		.into();
	}

	if is_error {
		"[tool error] (no tool output)"
	} else {
		"(no tool output)"
	}
	.into()
}

fn map_tool_choice(choice: &Value) -> PiResult<Value> {
	if let Some(choice @ ("auto" | "none" | "any" | "required")) = choice.as_str() {
		return Ok(Value::String(choice.into()));
	}
	let mut mapped_function = Map::new();
	if let Some(name) = read_property(choice.get("function"), "name")? {
		mapped_function.insert("name".into(), name.clone());
	}
	Ok(json!({ "type": "function", "function": mapped_function }))
}

fn map_chat_stop_reason(reason: &str) -> (StopReason, Option<String>) {
	match reason {
		"stop" => (StopReason::Stop, None),
		"length" | "model_length" => (StopReason::Length, None),
		"tool_calls" => (StopReason::ToolUse, None),
		// Mistral reports transient server failures this way; "server error" makes the message retryable.
		"error" => (
			StopReason::Error,
			Some("Provider stopped with: error (server error)".into()),
		),
		_ => (StopReason::Error, Some(format!("Provider stopped with: {reason}"))),
	}
}
