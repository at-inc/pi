//! Port of `packages/ai/src/api/pi-messages.ts`.
//! PORT-STATUS: reviewed
//!
//! pi-messages API implementation.
//!
//! Streams pi's own message protocol directly to a backend: the request is a single POST of
//! `{ model, context, options }` to `<baseUrl>/messages`, the response is an SSE stream of serialized
//! assistant-message events plus a terminal `done`/`error` event. This is the wire protocol spoken by the Radius
//! gateway, but any backend implementing it can be used, e.g. via a models.json custom provider with
//! `"api": "pi-messages"`.
//!
//! Differences from the TS:
//! - Wire events are JSON values. The TS edits content blocks with `Object.assign` and string concatenation; the
//!   port applies the same edits to the block's JSON and reads it back as a typed block, so a wire value that does not
//!   fit the block type (a non-string `content`, a `toolCall` without `id`) fails the stream with the deserialization
//!   message instead of storing the odd value. A `toolcall_end` `toolCall` that is not an object assigns nothing.
//! - Converted events are typed: fields the wire event carries beyond the `AssistantMessageEvent` shape are not
//!   forwarded (`onProviderStreamEvent` still sees the raw wire event), and an event with an unknown `type` is not
//!   pushed to the stream (the TS pushes it as it came). A `toolcall_end` on a block that is not a tool call fails the
//!   stream; the TS returns that block as the `toolCall`.
//! - A `done`/`error` event whose `reason` is not a stop reason fails the stream; a missing or malformed `usage`
//!   keeps the empty usage where the TS stores `undefined`; non-string `responseId`, `errorMessage`, `responseModel`
//!   and `providerThinkingLevel` are dropped.
//! - `streamSimple`'s `debug` comes from the `SimpleStreamOptions` extra keys in TS; `SimpleStreamOptions` has none in
//!   Rust, so `stream_simple` never sets it. `stream` reads `reasoning`, `toolChoice` and `debug` from
//!   `ProviderStreamOptions.extra`.
//! - A `text_end`/`thinking_end` without `content` fails the stream; the TS stores `text`/`thinking: undefined`.
//! - The payload's `context` is serialized from typed messages, so its keys follow the struct order rather than the
//!   order of the caller's objects.
//! - `response.body` is null only for the null-body statuses 204 and 205: a `fetch` override cannot return a 2xx
//!   response without a body.
//! - The TS async body starts inside `stream()`; here it runs on the host executor, so `onPayload` is first called
//!   once the caller yields.

use std::collections::HashMap;
use std::rc::Rc;

use futures::StreamExt;
use pi_host::js::{json_stringify, number_to_string, utf16_len};
use pi_host::{AbortSignal, HttpRequest, PiError, PiResult};
use serde_json::{Map, Value, json};

use crate::types::{
	AssistantContent, AssistantMessage, AssistantMessageEvent, CacheRetention, Model, ProviderEnv, ProviderHeaders,
	ProviderResponse, ProviderStreamOptions, ProviderStreams, SharedAssistantMessage, SimpleStreamOptions, StopReason,
	StreamOptions, TranscriptContext, Usage,
};
use crate::utils::diagnostics::{
	AssistantMessageDiagnostic, append_assistant_message_diagnostic, create_assistant_message_diagnostic,
};
use crate::utils::event_stream::AssistantMessageEventStream;
use crate::utils::headers::{headers_to_record, provider_headers_to_record};
use crate::utils::identity::{ClientIdentityTarget, get_identity_headers};
use crate::utils::json_parse::json_parse;
use crate::utils::provider_env::get_provider_env_value;
use crate::utils::streaming_json::ToolCallArgumentsStream;
use crate::utils::text::{js_trim, owned_slice};

/// `PiMessagesOptions`: `StreamOptions` plus `reasoning`, `toolChoice` and `debug` as their JSON values.
#[derive(Clone, Default)]
pub struct PiMessagesOptions {
	pub base: StreamOptions,
	pub reasoning: Option<Value>,
	pub tool_choice: Option<Value>,
	/// Ask the backend for debug metadata (e.g. routing response headers).
	pub debug: Option<Value>,
}

const RESPONSE_ERROR_NAME: &str = "PiMessagesResponseError";

/// The module's `ProviderStreams`, for `pi_messages_lazy`.
pub fn pi_messages_streams() -> Rc<dyn ProviderStreams> {
	Rc::new(PiMessagesStreams)
}

struct PiMessagesStreams;

impl ProviderStreams for PiMessagesStreams {
	fn stream(
		&self,
		model: &Model,
		context: &TranscriptContext,
		options: Option<ProviderStreamOptions>,
	) -> PiResult<AssistantMessageEventStream> {
		let options = options.map(|options| PiMessagesOptions {
			reasoning: options.extra.get("reasoning").cloned(),
			tool_choice: options.extra.get("toolChoice").cloned(),
			debug: options.extra.get("debug").cloned(),
			base: options.base,
		});
		Ok(stream(model, context, options))
	}

	fn stream_simple(
		&self,
		model: &Model,
		context: &TranscriptContext,
		options: Option<SimpleStreamOptions>,
	) -> PiResult<AssistantMessageEventStream> {
		Ok(stream_simple(model, context, options))
	}
}

/// `streamSimple`: `stream` with the simple options' `reasoning` and `toolChoice`.
pub fn stream_simple(
	model: &Model,
	context: &TranscriptContext,
	options: Option<SimpleStreamOptions>,
) -> AssistantMessageEventStream {
	let options = options.unwrap_or_default();
	stream(
		model,
		context,
		Some(PiMessagesOptions {
			reasoning: options.reasoning.as_ref().map(|reasoning| json!(reasoning)),
			tool_choice: options.tool_choice.as_ref().map(|tool_choice| json!(tool_choice)),
			debug: None,
			base: options.base,
		}),
	)
}

/// `stream`: POSTs the request and converts the backend's events.
pub fn stream(
	model: &Model,
	context: &TranscriptContext,
	options: Option<PiMessagesOptions>,
) -> AssistantMessageEventStream {
	let event_stream = AssistantMessageEventStream::new();
	let mut converter = EventConverter::new(model);
	let (target, model, context) = (event_stream.clone(), model.clone(), context.clone());
	let options = options.unwrap_or_default();
	pi_host::spawn_local(async move {
		if let Err(error) = run(&model, &context, &options, &target, &mut converter).await {
			let aborted = options.base.signal.as_ref().is_some_and(AbortSignal::aborted);
			target.push(create_error_event(&model, &error, aborted));
		}
	});
	event_stream
}

async fn run(
	model: &Model,
	context: &TranscriptContext,
	options: &PiMessagesOptions,
	event_stream: &AssistantMessageEventStream,
	converter: &mut EventConverter,
) -> PiResult<()> {
	let Some(api_key) = options.base.api_key.as_deref().filter(|key| !key.is_empty()) else {
		return Err(PiError::new(format!(
			"No API key provided for provider \"{}\"",
			model.provider
		)));
	};

	let mut url = url::Url::parse(&format!("{}/messages", model.base_url.trim_end_matches('/')))
		.map_err(|_| PiError::type_error("Invalid URL"))?;
	if options.debug.as_ref().is_some_and(truthy) {
		set_search_param(&mut url, "debug", "1");
	}

	let mut request_options = Map::new();
	let mut set = |key: &str, value: Option<Value>| {
		if let Some(value) = value {
			request_options.insert(key.into(), value);
		}
	};
	set("temperature", options.base.temperature.map(Value::from));
	set("maxTokens", options.base.max_tokens.map(Value::from));
	set("reasoning", options.reasoning.clone());
	set(
		"cacheRetention",
		resolve_cache_retention(options.base.cache_retention.as_ref(), options.base.env.as_ref())
			.map(|value| json!(value)),
	);
	set("sessionId", options.base.session_id.clone().map(Value::String));
	set("toolChoice", options.tool_choice.clone());
	let mut payload = json!({ "model": model.id, "context": context, "options": request_options });
	if let Some(on_payload) = &options.base.on_payload
		&& let Some(next_payload) = on_payload(payload.clone(), model).await?
	{
		payload = next_payload;
	}

	let identity_headers: ProviderHeaders = get_identity_headers(
		ClientIdentityTarget {
			provider: &model.provider,
			base_url: &model.base_url,
		},
		options.base.identity.as_ref(),
	)?
	.into_iter()
	.map(|(name, value)| (name, Some(value)))
	.collect();
	let default_headers = ProviderHeaders::from([
		("authorization".to_string(), Some(format!("Bearer {api_key}"))),
		("accept".to_string(), Some("text/event-stream".to_string())),
		("content-type".to_string(), Some("application/json".to_string())),
	]);
	let mut request = HttpRequest::new("POST", url.to_string());
	request.headers = provider_headers_to_record(&[
		Some(&identity_headers),
		Some(&default_headers),
		options.base.headers.as_ref(),
	])
	.unwrap_or_default();
	request.body = Some(json_stringify(&payload).into());
	request.signal = options.base.signal.clone();
	let response = pi_host::fetch(request, options.base.fetch.as_ref()).await?;

	if let Some(on_response) = &options.base.on_response {
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
		let (status, status_text) = (response.status, response.status_text.clone());
		let body = response.text().await?;
		return Err(create_response_error(model, &url, status, &status_text, &body));
	}

	// `fetch` gives a null-body status a null `response.body`.
	if matches!(response.status, 204 | 205) {
		return Err(PiError::new(format!("{} response has no body", model.provider)));
	}

	let mut reader = EventReader::default();
	let mut body = response.body;
	loop {
		let chunk = match body.next().await {
			Some(chunk) => Some(chunk?),
			None => None,
		};
		let done = chunk.is_none();
		for raw in reader.push(chunk.as_deref(), done) {
			let Some(pi_event) = parse_event(&raw)? else { continue };
			if let Some(on_provider_stream_event) = &options.base.on_provider_stream_event {
				on_provider_stream_event(&pi_event, model).await?;
			}
			let Some(event) = converter.convert(&pi_event)? else {
				continue;
			};
			let terminal = matches!(
				event,
				AssistantMessageEvent::Done { .. } | AssistantMessageEvent::Error { .. }
			);
			event_stream.push(event);
			if terminal {
				return Ok(());
			}
			// The TS reads events through an async generator, so a consumer runs between two events of one chunk.
			yield_now().await;
		}
		if done {
			break;
		}
	}

	Err(PiError::new(format!(
		"{} stream ended without a terminal event",
		model.provider
	)))
}

/// Lets every task that is already woken run before this one continues.
async fn yield_now() {
	let mut yielded = false;
	futures::future::poll_fn(|context| {
		if yielded {
			return std::task::Poll::Ready(());
		}
		yielded = true;
		context.waker().wake_by_ref();
		std::task::Poll::Pending
	})
	.await;
}

/// `url.searchParams.set(name, value)`: replaces the first pair with that name and drops the others, or appends.
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

fn resolve_cache_retention(
	cache_retention: Option<&CacheRetention>,
	env: Option<&ProviderEnv>,
) -> Option<CacheRetention> {
	if let Some(cache_retention) = cache_retention {
		return Some(*cache_retention);
	}
	// Backend defaults apply when unset; only the legacy env opt-in is mapped.
	(get_provider_env_value("PI_CACHE_RETENTION", env).as_deref() == Some("long"))
		.then(|| serde_json::from_value(json!("long")).expect("long is a cache retention"))
}

/// `createPiMessagesResponseError`: a `PiMessagesResponseError` whose `code` and `diagnosticDetails` live in
/// `data`.
fn create_response_error(model: &Model, url: &url::Url, status: u16, status_text: &str, body: &str) -> PiError {
	let error_body = parse_error_body(body);
	let error = error_body
		.as_ref()
		.and_then(|body| body.get("error"))
		.and_then(Value::as_object);
	let code = error.and_then(|error| error.get("code")).and_then(Value::as_str);
	let message = error.and_then(|error| error.get("message")).and_then(Value::as_str);
	let code_suffix = code
		.filter(|code| !code.is_empty())
		.map(|code| format!(" ({code})"))
		.unwrap_or_default();
	let text = format!("{status} {status_text}: {}{code_suffix}", message.unwrap_or(body));

	let mut details = Map::new();
	details.insert("version".into(), json!(1));
	details.insert("provider".into(), json!(model.provider));
	details.insert("model".into(), json!(model.id));
	details.insert("url".into(), json!(url.to_string()));
	details.insert("status".into(), json!(status));
	details.insert("statusText".into(), json!(status_text));
	if let Some(error) = error {
		details.insert("error".into(), Value::Object(error.clone()));
	}
	if error_body.is_none() {
		details.insert("body".into(), json!(truncate_diagnostic_string(body)));
	}
	details.insert("timestampMs".into(), json!(pi_host::now_ms()));

	let mut data = Map::new();
	if let Some(code) = code {
		data.insert("code".into(), json!(code));
	}
	data.insert("diagnosticDetails".into(), Value::Object(details));
	PiError::named(RESPONSE_ERROR_NAME, text).with_data(Value::Object(data))
}

/// The parsed body when its `error` is a non-array object.
fn parse_error_body(body: &str) -> Option<Value> {
	let parsed = json_parse(body).ok()?;
	parsed
		.get("error")
		.filter(|error| error.is_object())
		.is_some()
		.then_some(parsed)
}

fn truncate_diagnostic_string(value: &str) -> String {
	const MAX_LENGTH: usize = 8192;
	if utf16_len(value) > MAX_LENGTH {
		format!("{}…", owned_slice(value, 0, Some(MAX_LENGTH)))
	} else {
		value.to_string()
	}
}

fn create_empty_usage() -> Usage {
	Usage::default()
}

fn create_error_event(model: &Model, error: &PiError, aborted: bool) -> AssistantMessageEvent {
	let reason = if aborted {
		StopReason::Aborted
	} else {
		StopReason::Error
	};
	let mut message = AssistantMessage {
		content: Vec::new(),
		api: model.api.clone(),
		provider: model.provider.clone(),
		model: model.id.clone(),
		usage: create_empty_usage(),
		stop_reason: reason,
		error_message: Some(error.message.clone()),
		timestamp: pi_host::now_ms(),
		..AssistantMessage::default()
	};
	if !aborted && error.name == RESPONSE_ERROR_NAME {
		let details = error
			.data
			.as_ref()
			.and_then(|data| data.get("diagnosticDetails"))
			.and_then(Value::as_object)
			.cloned();
		append_assistant_message_diagnostic(
			&mut message,
			create_assistant_message_diagnostic("pi_messages_response_failure", error, details),
		);
	}
	AssistantMessageEvent::Error {
		reason,
		error: Rc::new(std::cell::RefCell::new(message)),
	}
}

/// `readPiMessagesEvents`'s framing: streaming UTF-8 decoding (a leading BOM dropped, invalid bytes replaced), CRLF
/// normalized to LF, and events split on blank lines; the rest of the buffer is one more event at the end.
#[derive(Default)]
struct EventReader {
	pending: Vec<u8>,
	started: bool,
	buffer: String,
}

impl EventReader {
	/// Feeds `chunk` (`None` at end of stream, with `done`) and returns the raw events it completed.
	fn push(&mut self, chunk: Option<&[u8]>, done: bool) -> Vec<String> {
		if let Some(chunk) = chunk {
			self.pending.extend_from_slice(chunk);
		}
		let decoded = self.decode(done);
		self.buffer.push_str(&decoded);
		self.buffer = self.buffer.replace("\r\n", "\n");
		let mut events = Vec::new();
		while let Some(split) = self.buffer.find("\n\n") {
			events.push(self.buffer[..split].to_string());
			self.buffer = self.buffer[split + 2..].to_string();
		}
		if done && !js_trim(&self.buffer).is_empty() {
			events.push(std::mem::take(&mut self.buffer));
		}
		events
	}

	fn decode(&mut self, done: bool) -> String {
		let mut decoded = String::new();
		loop {
			match std::str::from_utf8(&self.pending) {
				Ok(text) => {
					decoded.push_str(text);
					self.pending.clear();
					break;
				}
				Err(error) => {
					let valid = error.valid_up_to();
					decoded.push_str(std::str::from_utf8(&self.pending[..valid]).expect("the prefix is valid UTF-8"));
					match error.error_len() {
						Some(length) => {
							decoded.push('\u{FFFD}');
							self.pending.drain(..valid + length);
						}
						None if done => {
							decoded.push('\u{FFFD}');
							self.pending.clear();
							break;
						}
						None => {
							self.pending.drain(..valid);
							break;
						}
					}
				}
			}
		}
		if !self.started && !decoded.is_empty() {
			self.started = true;
			if let Some(rest) = decoded.strip_prefix('\u{FEFF}') {
				return rest.to_string();
			}
		}
		decoded
	}
}

/// `parsePiMessagesEvent`: the first `data:` line's JSON, or `None` for no data and `[DONE]`. A falsy JSON value is
/// skipped like the TS `if (event)`.
fn parse_event(raw: &str) -> PiResult<Option<Value>> {
	let Some(data) = raw
		.split('\n')
		.find(|line| line.starts_with("data:"))
		.map(|line| js_trim(&line[5..]))
	else {
		return Ok(None);
	};
	if data.is_empty() || data == "[DONE]" {
		return Ok(None);
	}
	Ok(Some(json_parse(data)?).filter(truthy))
}

fn truthy(value: &Value) -> bool {
	match value {
		Value::Null => false,
		Value::Bool(value) => *value,
		Value::Number(number) => number.as_f64().is_some_and(|number| number != 0.0),
		Value::String(text) => !text.is_empty(),
		Value::Array(_) | Value::Object(_) => true,
	}
}

/// JS `String(value)`, with `undefined` for a missing value.
fn js_string(value: Option<&Value>) -> String {
	match value {
		None => "undefined".into(),
		Some(Value::Null) => "null".into(),
		Some(Value::Bool(value)) => value.to_string(),
		Some(Value::Number(number)) => number_to_string(number.as_f64().unwrap_or(f64::NAN)),
		Some(Value::String(text)) => text.clone(),
		Some(Value::Array(items)) => items
			.iter()
			.map(|item| {
				if item.is_null() {
					String::new()
				} else {
					js_string(Some(item))
				}
			})
			.collect::<Vec<_>>()
			.join(","),
		Some(Value::Object(_)) => "[object Object]".into(),
	}
}

/// `createEventConverter(model)`: the partial message every event shares, and the tool argument streams.
struct EventConverter {
	partial: SharedAssistantMessage,
	tool_arguments: HashMap<usize, ToolArguments>,
}

/// One `toolArguments` entry. The stream updates the tool call block's arguments in place; `detached` holds them once
/// a later `*_start` replaced the block, because TS keeps assigning the stream's own object to whatever block sits at
/// the index.
#[derive(Default)]
struct ToolArguments {
	stream: ToolCallArgumentsStream,
	detached: Option<Map<String, Value>>,
}

impl EventConverter {
	fn new(model: &Model) -> Self {
		Self {
			partial: Rc::new(std::cell::RefCell::new(AssistantMessage {
				content: Vec::new(),
				api: model.api.clone(),
				provider: model.provider.clone(),
				model: model.id.clone(),
				usage: create_empty_usage(),
				stop_reason: StopReason::Pending,
				timestamp: pi_host::now_ms(),
				..AssistantMessage::default()
			})),
			tool_arguments: HashMap::new(),
		}
	}

	/// Applies one wire event to the partial message. `None` for an event the stream has no type for.
	fn convert(&mut self, event: &Value) -> PiResult<Option<AssistantMessageEvent>> {
		let fields = match event {
			Value::Object(fields) => fields,
			// `"contentIndex" in []` is false and an array has no `type`.
			Value::Array(_) => return Ok(None),
			_ => {
				return Err(PiError::type_error(format!(
					"Cannot use 'in' operator to search for 'contentIndex' in {}",
					js_string(Some(event))
				)));
			}
		};
		let event_type = fields.get("type");
		let index = match fields.get("contentIndex") {
			Some(index) => Some(self.content_index(index, event_type)?),
			None => None,
		};
		let Some(Value::String(event_type)) = event_type else {
			return Ok(None);
		};
		let partial = self.partial.clone();
		let at = || index.expect("events with a content block carry contentIndex");
		let event = match event_type.as_str() {
			"done" => {
				let reason = self.finish(fields, false)?;
				AssistantMessageEvent::Done {
					reason,
					message: partial,
				}
			}
			"error" => {
				let reason = self.finish(fields, true)?;
				AssistantMessageEvent::Error { reason, error: partial }
			}
			"start" => AssistantMessageEvent::Start { partial },
			"text_start" => {
				self.set_block(at(), json!({ "type": "text", "text": "" }))?;
				AssistantMessageEvent::TextStart {
					content_index: at(),
					partial,
				}
			}
			"text_delta" => {
				let delta = js_string(fields.get("delta"));
				self.append_text(at(), "text", &delta)?;
				AssistantMessageEvent::TextDelta {
					content_index: at(),
					delta,
					partial,
				}
			}
			"text_end" => {
				self.assign(
					at(),
					[
						("text", fields.get("content")),
						("textSignature", fields.get("contentSignature")),
					],
				)?;
				AssistantMessageEvent::TextEnd {
					content_index: at(),
					content: js_string(fields.get("content")),
					partial,
				}
			}
			"thinking_start" => {
				self.set_block(at(), json!({ "type": "thinking", "thinking": "" }))?;
				AssistantMessageEvent::ThinkingStart {
					content_index: at(),
					partial,
				}
			}
			"thinking_delta" => {
				let delta = js_string(fields.get("delta"));
				self.append_text(at(), "thinking", &delta)?;
				AssistantMessageEvent::ThinkingDelta {
					content_index: at(),
					delta,
					partial,
				}
			}
			"thinking_end" => {
				self.assign(
					at(),
					[
						("thinking", fields.get("content")),
						("thinkingSignature", fields.get("contentSignature")),
						("redacted", fields.get("redacted")),
					],
				)?;
				AssistantMessageEvent::ThinkingEnd {
					content_index: at(),
					content: js_string(fields.get("content")),
					partial,
				}
			}
			"toolcall_start" => {
				let mut block = Map::new();
				block.insert("type".into(), json!("toolCall"));
				for (key, field) in [("id", "id"), ("name", "toolName")] {
					if let Some(value) = fields.get(field) {
						block.insert(key.into(), value.clone());
					}
				}
				block.insert("arguments".into(), json!({}));
				self.set_block(at(), Value::Object(block))?;
				self.tool_arguments.insert(at(), ToolArguments::default());
				AssistantMessageEvent::ToolcallStart {
					content_index: at(),
					partial,
				}
			}
			"toolcall_delta" => {
				let delta = js_string(fields.get("delta"));
				if let Some(arguments) = self.tool_arguments.get_mut(&at()) {
					let mut partial = self.partial.borrow_mut();
					if let (AssistantContent::ToolCall(tool_call), None) =
						(&mut partial.content[at()], &arguments.detached)
					{
						arguments.stream.append(&delta, &mut tool_call.arguments);
					} else {
						drop(partial);
						let detached = match &mut arguments.detached {
							Some(detached) => {
								arguments.stream.append(&delta, detached);
								detached
							}
							// The replaced block took the arguments so far with it: rebuild them from the whole text.
							None => {
								let text = format!("{}{delta}", arguments.stream.text());
								arguments.stream = ToolCallArgumentsStream::new();
								let mut detached = Map::new();
								arguments.stream.append(&text, &mut detached);
								arguments.detached.insert(detached)
							}
						};
						let value = Value::Object(detached.clone());
						self.assign(at(), [("arguments", Some(&value))])?;
					}
				}
				AssistantMessageEvent::ToolcallDelta {
					content_index: at(),
					delta,
					partial: self.partial.clone(),
				}
			}
			"toolcall_end" => {
				if let Some(Value::Object(tool_call)) = fields.get("toolCall") {
					self.assign(at(), tool_call.iter().map(|(key, value)| (key.as_str(), Some(value))))?;
				}
				self.tool_arguments.remove(&at());
				let tool_call = match &self.partial.borrow().content[at()] {
					AssistantContent::ToolCall(tool_call) => tool_call.clone(),
					_ => return Err(PiError::new("pi-messages toolcall_end event did not end a tool call")),
				};
				AssistantMessageEvent::ToolcallEnd {
					content_index: at(),
					tool_call,
					partial,
				}
			}
			_ => return Ok(None),
		};
		Ok(Some(event))
	}

	/// Checks `contentIndex`: an integer naming the next block for a `*_start` event, an existing block otherwise.
	fn content_index(&self, index: &Value, event_type: Option<&Value>) -> PiResult<usize> {
		let event_type = match event_type {
			Some(Value::String(event_type)) => event_type,
			None => {
				return Err(PiError::type_error(
					"Cannot read properties of undefined (reading 'endsWith')",
				));
			}
			Some(Value::Null) => {
				return Err(PiError::type_error(
					"Cannot read properties of null (reading 'endsWith')",
				));
			}
			Some(_) => return Err(PiError::type_error("event.type.endsWith is not a function")),
		};
		let length = self.partial.borrow().content.len() as f64;
		let limit = if event_type.ends_with("_start") {
			length
		} else {
			length - 1.0
		};
		match index.as_f64() {
			Some(number) if index.is_number() && number.fract() == 0.0 && number >= 0.0 && number <= limit => {
				Ok(number as usize)
			}
			_ => Err(PiError::new(format!(
				"Invalid contentIndex {} in pi-messages {event_type} event",
				json_stringify(index)
			))),
		}
	}

	/// `Object.assign(partial, { stopReason, usage, responseId, errorMessage? })` plus the optional fields and the
	/// rewrite diagnostic.
	fn finish(&mut self, fields: &Map<String, Value>, error: bool) -> PiResult<StopReason> {
		let reason: StopReason = serde_json::from_value(fields.get("reason").cloned().unwrap_or(Value::Null))
			.map_err(|parse_error| PiError::new(format!("Invalid pi-messages stop reason: {parse_error}")))?;
		let text = |key: &str| fields.get(key).and_then(Value::as_str).map(str::to_string);
		let mut partial = self.partial.borrow_mut();
		partial.stop_reason = reason;
		if let Some(usage) = fields
			.get("usage")
			.and_then(|usage| serde_json::from_value(usage.clone()).ok())
		{
			partial.usage = usage;
		}
		if error {
			partial.error_message = text("errorMessage");
		}
		partial.response_id = text("responseId");
		if fields.contains_key("responseModel") {
			partial.response_model = text("responseModel");
		}
		if fields.contains_key("providerThinkingLevel") {
			partial.provider_thinking_level = text("providerThinkingLevel");
		}
		if let Some(rewrite) = fields.get("rewrite").filter(|rewrite| truthy(rewrite)) {
			let details: Map<String, Value> = match rewrite {
				Value::Object(rewrite) => rewrite.clone(),
				Value::Array(items) => items
					.iter()
					.enumerate()
					.map(|(index, item)| (index.to_string(), item.clone()))
					.collect(),
				Value::String(text) => text
					.encode_utf16()
					.enumerate()
					.map(|(index, unit)| (index.to_string(), json!(String::from_utf16_lossy(&[unit]))))
					.collect(),
				_ => Map::new(),
			};
			append_assistant_message_diagnostic(
				&mut partial,
				AssistantMessageDiagnostic {
					r#type: "pi_messages_rewrite".into(),
					timestamp: pi_host::now_ms(),
					error: None,
					details: Some(details),
				},
			);
		}
		Ok(reason)
	}

	/// `partial.content[index] = block`.
	fn set_block(&mut self, index: usize, block: Value) -> PiResult<()> {
		let block = read_block(block)?;
		let mut partial = self.partial.borrow_mut();
		if index == partial.content.len() {
			partial.content.push(block);
		} else {
			partial.content[index] = block;
		}
		Ok(())
	}

	/// `(partial.content[index] as { [field]: string })[field] += delta`.
	fn append_text(&mut self, index: usize, field: &str, delta: &str) -> PiResult<()> {
		let mut partial = self.partial.borrow_mut();
		match (&mut partial.content[index], field) {
			(AssistantContent::Text(text), "text") => text.text.push_str(delta),
			(AssistantContent::Thinking(thinking), "thinking") => thinking.thinking.push_str(delta),
			(block, _) => {
				let mut fields = block_fields(block);
				let current = js_string(fields.get(field));
				fields.insert(field.into(), json!(format!("{current}{delta}")));
				*block = read_block(Value::Object(fields))?;
			}
		}
		Ok(())
	}

	/// `Object.assign(partial.content[index], fields)`: a missing value stores `undefined`, which drops the key.
	fn assign<'a>(
		&mut self,
		index: usize,
		assigned: impl IntoIterator<Item = (&'a str, Option<&'a Value>)>,
	) -> PiResult<()> {
		let mut partial = self.partial.borrow_mut();
		let block = &mut partial.content[index];
		let mut fields = block_fields(block);
		for (key, value) in assigned {
			match value {
				Some(value) => {
					fields.insert(key.into(), value.clone());
				}
				None => {
					fields.shift_remove(key);
				}
			}
		}
		*block = read_block(Value::Object(fields))?;
		Ok(())
	}
}

fn block_fields(block: &AssistantContent) -> Map<String, Value> {
	match serde_json::to_value(block) {
		Ok(Value::Object(fields)) => fields,
		_ => unreachable!("a content block serializes to an object"),
	}
}

fn read_block(block: Value) -> PiResult<AssistantContent> {
	serde_json::from_value(block).map_err(|error| PiError::new(format!("Invalid pi-messages content block: {error}")))
}
