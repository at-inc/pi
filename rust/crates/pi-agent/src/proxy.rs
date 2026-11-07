//! Port of `packages/agent/src/proxy.ts`.
//! PORT-STATUS: ported
//!
//! Proxy stream function for apps that route LLM calls through a server.
//! The server manages auth and proxies requests to LLM providers.
//!
//! Rust differences:
//! - Proxy events are deserialized into [`ProxyAssistantMessageEvent`]. An event with an unknown `type` is skipped
//!   without the TS `console.warn` (the core does no I/O); an event of a known type with missing or mistyped fields
//!   fails the stream with the deserialization error, where TS reads `undefined` from it. Invalid JSON fails the
//!   stream like TS, with serde's message instead of V8's `JSON.parse` message.
//! - A content index past the end of `partial.content` appends the block; TS leaves a hole in the array.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use futures::StreamExt;
use futures::future::Either;
use pi_ai::types::{
	AssistantContent, AssistantMessage, AssistantMessageEvent, CacheRetention, Model, ProviderHeaders,
	SamplingParams, SharedAssistantMessage, StopReason, TextContent, ThinkingBudgets, ThinkingContent,
	ThinkingLevel, ToolCall, TranscriptContext, Transport, Usage,
};
use pi_ai::utils::event_stream::{AssistantMessageEventStream, create_assistant_message_event_stream};
use pi_ai::utils::streaming_json::ToolCallArgumentsStream;
use pi_host::{AbortSignal, HttpRequest, PiError, PiResult};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

/// Proxy event types - server sends these with partial field stripped to reduce bandwidth.
#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", rename_all_fields = "camelCase")]
pub enum ProxyAssistantMessageEvent {
	Start,
	TextStart {
		content_index: usize,
	},
	TextDelta {
		content_index: usize,
		delta: String,
	},
	TextEnd {
		content_index: usize,
		#[serde(default)]
		content_signature: Option<String>,
	},
	ThinkingStart {
		content_index: usize,
	},
	ThinkingDelta {
		content_index: usize,
		delta: String,
	},
	ThinkingEnd {
		content_index: usize,
		#[serde(default)]
		content_signature: Option<String>,
	},
	ToolcallStart {
		content_index: usize,
		id: String,
		tool_name: String,
	},
	ToolcallDelta {
		content_index: usize,
		delta: String,
	},
	/// `toolCall` stays raw JSON: `toolcall_end` assigns its fields onto the streamed block.
	ToolcallEnd {
		content_index: usize,
		tool_call: Map<String, Value>,
	},
	Done {
		reason: StopReason,
		usage: Usage,
		#[serde(default)]
		provider_thinking_level: Option<String>,
	},
	Error {
		reason: StopReason,
		#[serde(default)]
		error_message: Option<String>,
		usage: Usage,
		#[serde(default)]
		provider_thinking_level: Option<String>,
	},
}

const PROXY_EVENT_TYPES: [&str; 12] = [
	"start",
	"text_start",
	"text_delta",
	"text_end",
	"thinking_start",
	"thinking_delta",
	"thinking_end",
	"toolcall_start",
	"toolcall_delta",
	"toolcall_end",
	"done",
	"error",
];

/// `ProxyStreamOptions extends ProxySerializableStreamOptions`: the serializable stream options are repeated here.
#[derive(Clone, Debug, Default)]
pub struct ProxyStreamOptions {
	pub temperature: Option<f64>,
	pub sampling_params: Option<SamplingParams>,
	pub max_tokens: Option<f64>,
	pub reasoning: Option<ThinkingLevel>,
	pub cache_retention: Option<CacheRetention>,
	pub session_id: Option<String>,
	pub headers: Option<ProviderHeaders>,
	pub metadata: Option<Map<String, Value>>,
	pub transport: Option<Transport>,
	pub thinking_budgets: Option<ThinkingBudgets>,
	pub max_retry_delay_ms: Option<f64>,
	/// Local abort signal for the proxy request
	pub signal: Option<AbortSignal>,
	/// Auth token for the proxy server
	pub auth_token: String,
	/// Proxy server URL (e.g., "https://genai.example.com")
	pub proxy_url: String,
}

/// Stream function that proxies through a server instead of calling LLM providers directly.
/// The server strips the partial field from delta events to reduce bandwidth.
/// We reconstruct the partial message client-side.
///
/// Use this as the `streamFn` option when creating an Agent that needs to go through a proxy.
fn build_proxy_request_options(options: &ProxyStreamOptions) -> Map<String, Value> {
	let mut request_options = Map::new();
	insert_defined(&mut request_options, "temperature", &options.temperature);
	insert_defined(&mut request_options, "samplingParams", &options.sampling_params);
	insert_defined(&mut request_options, "maxTokens", &options.max_tokens);
	insert_defined(&mut request_options, "reasoning", &options.reasoning);
	insert_defined(&mut request_options, "cacheRetention", &options.cache_retention);
	insert_defined(&mut request_options, "sessionId", &options.session_id);
	insert_defined(&mut request_options, "headers", &options.headers);
	insert_defined(&mut request_options, "metadata", &options.metadata);
	insert_defined(&mut request_options, "transport", &options.transport);
	insert_defined(&mut request_options, "thinkingBudgets", &options.thinking_budgets);
	insert_defined(&mut request_options, "maxRetryDelayMs", &options.max_retry_delay_ms);
	request_options
}

/// A property of an object literal: `JSON.stringify` drops it when it is undefined.
fn insert_defined<T: Serialize>(object: &mut Map<String, Value>, key: &str, value: &Option<T>) {
	if let Some(value) = value {
		object.insert(key.to_string(), serde_json::to_value(value).expect("stream options serialize to JSON"));
	}
}

/// `streamProxy(model, context, options)`.
pub fn stream_proxy(model: &Model, context: &TranscriptContext, options: ProxyStreamOptions) -> AssistantMessageEventStream {
	let stream = create_assistant_message_event_stream();

	// Initialize the partial message that we'll build up from events
	let partial: SharedAssistantMessage = Rc::new(RefCell::new(AssistantMessage {
		stop_reason: StopReason::Pending,
		content: Vec::new(),
		api: model.api.clone(),
		provider: model.provider.clone(),
		model: model.id.clone(),
		usage: Usage::default(),
		timestamp: pi_host::now_ms(),
		..AssistantMessage::default()
	}));

	let body = pi_host::js::json_stringify(&serde_json::json!({
		"model": model,
		"context": context,
		"options": build_proxy_request_options(&options),
	}));
	let output = stream.clone();
	pi_host::spawn_local(async move {
		if let Err(error) = run_proxy_stream(body, &options, &partial, &output).await {
			let reason = if options.signal.as_ref().is_some_and(AbortSignal::aborted) {
				StopReason::Aborted
			} else {
				StopReason::Error
			};
			{
				let mut partial = partial.borrow_mut();
				partial.stop_reason = reason;
				partial.error_message = Some(error.message);
			}
			output.push(AssistantMessageEvent::Error {
				reason,
				error: partial.clone(),
			});
			output.end(None);
		}
	});

	stream
}

/// The `try` block of `streamProxy`.
async fn run_proxy_stream(
	body: String,
	options: &ProxyStreamOptions,
	partial: &SharedAssistantMessage,
	stream: &AssistantMessageEventStream,
) -> PiResult<()> {
	let mut request = HttpRequest::new("POST", format!("{}/api/stream", options.proxy_url));
	request.set_header("Authorization", format!("Bearer {}", options.auth_token));
	request.set_header("Content-Type", "application/json");
	request.body = Some(body.into());
	request.signal = options.signal.clone();
	let response = pi_host::fetch(request, None).await?;

	if !response.ok() {
		let mut error_message = format!("Proxy error: {} {}", response.status, response.status_text);
		if let Ok(Value::Object(error_data)) = response.json().await
			&& let Some(error) = error_data.get("error").filter(|error| is_truthy(error))
		{
			error_message = format!("Proxy error: {}", js_string(error));
		}
		return Err(PiError::new(error_message));
	}

	let mut reader = response.body;
	let mut decoder = Utf8StreamDecoder::default();
	let mut buffer = String::new();
	let mut saw_terminal_event = false;
	// Argument streams of open tool calls by content index, kept off the content blocks.
	let mut tool_arguments: HashMap<usize, ToolCallArgumentsStream> = HashMap::new();

	let mut process_line = |line: &str| -> PiResult<()> {
		let Some(data) = line.strip_prefix("data: ") else {
			return Ok(());
		};
		let data = data.trim();
		if data.is_empty() {
			return Ok(());
		}
		let proxy_event: Value = serde_json::from_str(data).map_err(|error| PiError::named("SyntaxError", error.to_string()))?;
		let known = proxy_event
			.get("type")
			.and_then(Value::as_str)
			.is_some_and(|event_type| PROXY_EVENT_TYPES.contains(&event_type));
		if !known {
			return Ok(());
		}
		let proxy_event: ProxyAssistantMessageEvent = serde_json::from_value(proxy_event)?;
		if let Some(event) = process_proxy_event(proxy_event, partial, &mut tool_arguments)? {
			if matches!(
				event,
				AssistantMessageEvent::Done { .. } | AssistantMessageEvent::Error { .. }
			) {
				saw_terminal_event = true;
			}
			stream.push(event);
		}
		Ok(())
	};

	loop {
		let chunk = match &options.signal {
			Some(signal) => {
				let aborted = signal.aborted_future();
				futures::pin_mut!(aborted);
				match futures::future::select(reader.next(), aborted).await {
					Either::Left((chunk, _)) => chunk,
					// Cancelling the reader ends the read loop like a finished body.
					Either::Right(_) => None,
				}
			}
			None => reader.next().await,
		};
		let Some(chunk) = chunk else {
			break;
		};
		let chunk = chunk?;

		if options.signal.as_ref().is_some_and(AbortSignal::aborted) {
			return Err(PiError::new("Request aborted by user"));
		}

		buffer.push_str(&decoder.decode(&chunk));
		let mut lines: Vec<String> = buffer.split('\n').map(str::to_string).collect();
		buffer = lines.pop().unwrap_or_default();

		for line in &lines {
			process_line(line)?;
		}
	}

	if options.signal.as_ref().is_some_and(AbortSignal::aborted) {
		return Err(PiError::new("Request aborted by user"));
	}

	// The final event may not be newline-terminated; flush the decoder and
	// process whatever is left in the buffer.
	buffer.push_str(&decoder.finish());
	if !buffer.is_empty() {
		process_line(&buffer)?;
	}

	if !saw_terminal_event {
		// A clean EOF without a done/error event means the server dropped the
		// response mid-stream. Surface it as an error instead of leaving
		// consumers waiting on a result that never arrives.
		{
			let mut partial = partial.borrow_mut();
			partial.stop_reason = StopReason::Error;
			partial.error_message = Some("Connection closed by proxy server before the response completed".into());
		}
		stream.push(AssistantMessageEvent::Error {
			reason: StopReason::Error,
			error: partial.clone(),
		});
	}

	stream.end(None);
	Ok(())
}

/// JS truthiness of a JSON value.
fn is_truthy(value: &Value) -> bool {
	match value {
		Value::Null => false,
		Value::Bool(value) => *value,
		Value::Number(number) => number.as_f64().is_some_and(|number| number != 0.0 && !number.is_nan()),
		Value::String(text) => !text.is_empty(),
		Value::Array(_) | Value::Object(_) => true,
	}
}

/// JS `String(value)` of a JSON value.
fn js_string(value: &Value) -> String {
	match value {
		Value::Null => "null".into(),
		Value::Bool(value) => value.to_string(),
		Value::Number(number) => pi_host::js::number_to_string(number.as_f64().unwrap_or(f64::NAN)),
		Value::String(text) => text.clone(),
		Value::Array(items) => items
			.iter()
			.map(|item| if item.is_null() { String::new() } else { js_string(item) })
			.collect::<Vec<_>>()
			.join(","),
		Value::Object(_) => "[object Object]".into(),
	}
}

/// `new TextDecoder()` with `decode(chunk, { stream: true })`: lossy UTF-8 that keeps an incomplete trailing
/// sequence for the next chunk.
#[derive(Default)]
struct Utf8StreamDecoder {
	pending: Vec<u8>,
}

impl Utf8StreamDecoder {
	fn decode(&mut self, chunk: &[u8]) -> String {
		self.pending.extend_from_slice(chunk);
		let mut out = String::new();
		let mut rest: &[u8] = &self.pending;
		loop {
			match std::str::from_utf8(rest) {
				Ok(text) => {
					out.push_str(text);
					rest = &[];
					break;
				}
				Err(error) => {
					let (valid, after) = rest.split_at(error.valid_up_to());
					out.push_str(std::str::from_utf8(valid).expect("valid_up_to bytes are UTF-8"));
					match error.error_len() {
						Some(length) => {
							out.push('\u{FFFD}');
							rest = &after[length..];
						}
						None => {
							rest = after;
							break;
						}
					}
				}
			}
		}
		self.pending = rest.to_vec();
		out
	}

	fn finish(&mut self) -> String {
		let pending = std::mem::take(&mut self.pending);
		String::from_utf8_lossy(&pending).into_owned()
	}
}

/// Process a proxy event and update the partial message.
fn process_proxy_event(
	proxy_event: ProxyAssistantMessageEvent,
	partial: &SharedAssistantMessage,
	tool_arguments: &mut HashMap<usize, ToolCallArgumentsStream>,
) -> PiResult<Option<AssistantMessageEvent>> {
	let event = match proxy_event {
		ProxyAssistantMessageEvent::Start => AssistantMessageEvent::Start {
			partial: partial.clone(),
		},

		ProxyAssistantMessageEvent::TextStart { content_index } => {
			set_content(
				partial,
				content_index,
				AssistantContent::Text(TextContent::default()),
			);
			AssistantMessageEvent::TextStart {
				content_index,
				partial: partial.clone(),
			}
		}

		ProxyAssistantMessageEvent::TextDelta { content_index, delta } => {
			match partial.borrow_mut().content.get_mut(content_index) {
				Some(AssistantContent::Text(content)) => content.text.push_str(&delta),
				_ => return Err(PiError::new("Received text_delta for non-text content")),
			}
			AssistantMessageEvent::TextDelta {
				content_index,
				delta,
				partial: partial.clone(),
			}
		}

		ProxyAssistantMessageEvent::TextEnd {
			content_index,
			content_signature,
		} => {
			let content = match partial.borrow_mut().content.get_mut(content_index) {
				Some(AssistantContent::Text(content)) => {
					content.text_signature = content_signature;
					content.text.clone()
				}
				_ => return Err(PiError::new("Received text_end for non-text content")),
			};
			AssistantMessageEvent::TextEnd {
				content_index,
				content,
				partial: partial.clone(),
			}
		}

		ProxyAssistantMessageEvent::ThinkingStart { content_index } => {
			set_content(
				partial,
				content_index,
				AssistantContent::Thinking(ThinkingContent::default()),
			);
			AssistantMessageEvent::ThinkingStart {
				content_index,
				partial: partial.clone(),
			}
		}

		ProxyAssistantMessageEvent::ThinkingDelta { content_index, delta } => {
			match partial.borrow_mut().content.get_mut(content_index) {
				Some(AssistantContent::Thinking(content)) => content.thinking.push_str(&delta),
				_ => return Err(PiError::new("Received thinking_delta for non-thinking content")),
			}
			AssistantMessageEvent::ThinkingDelta {
				content_index,
				delta,
				partial: partial.clone(),
			}
		}

		ProxyAssistantMessageEvent::ThinkingEnd {
			content_index,
			content_signature,
		} => {
			let content = match partial.borrow_mut().content.get_mut(content_index) {
				Some(AssistantContent::Thinking(content)) => {
					content.thinking_signature = content_signature;
					content.thinking.clone()
				}
				_ => return Err(PiError::new("Received thinking_end for non-thinking content")),
			};
			AssistantMessageEvent::ThinkingEnd {
				content_index,
				content,
				partial: partial.clone(),
			}
		}

		ProxyAssistantMessageEvent::ToolcallStart {
			content_index,
			id,
			tool_name,
		} => {
			set_content(
				partial,
				content_index,
				AssistantContent::ToolCall(ToolCall {
					id,
					name: tool_name,
					..ToolCall::default()
				}),
			);
			tool_arguments.insert(content_index, ToolCallArgumentsStream::new());
			AssistantMessageEvent::ToolcallStart {
				content_index,
				partial: partial.clone(),
			}
		}

		ProxyAssistantMessageEvent::ToolcallDelta { content_index, delta } => {
			let args = tool_arguments.get_mut(&content_index);
			match (partial.borrow_mut().content.get_mut(content_index), args) {
				(Some(AssistantContent::ToolCall(content)), Some(args)) => {
					content.arguments = (*args.append(&delta)).clone();
				}
				_ => return Err(PiError::new("Received toolcall_delta for non-toolCall content")),
			}
			AssistantMessageEvent::ToolcallDelta {
				content_index,
				delta,
				partial: partial.clone(),
			}
		}

		ProxyAssistantMessageEvent::ToolcallEnd {
			content_index,
			tool_call,
		} => {
			let tool_call = {
				let mut message = partial.borrow_mut();
				let Some(AssistantContent::ToolCall(content)) = message.content.get_mut(content_index) else {
					return Ok(None);
				};
				// Object.assign(content, proxyEvent.toolCall)
				let Value::Object(mut merged) = serde_json::to_value(&*content)? else {
					unreachable!("a tool call serializes to an object");
				};
				merged.extend(tool_call);
				*content = serde_json::from_value(Value::Object(merged))?;
				content.clone()
			};
			tool_arguments.remove(&content_index);
			AssistantMessageEvent::ToolcallEnd {
				content_index,
				tool_call,
				partial: partial.clone(),
			}
		}

		ProxyAssistantMessageEvent::Done {
			reason,
			usage,
			provider_thinking_level,
		} => {
			{
				let mut message = partial.borrow_mut();
				message.stop_reason = reason;
				message.usage = usage;
				if provider_thinking_level.is_some() {
					message.provider_thinking_level = provider_thinking_level;
				}
			}
			AssistantMessageEvent::Done {
				reason,
				message: partial.clone(),
			}
		}

		ProxyAssistantMessageEvent::Error {
			reason,
			error_message,
			usage,
			provider_thinking_level,
		} => {
			{
				let mut message = partial.borrow_mut();
				message.stop_reason = reason;
				message.error_message = error_message;
				message.usage = usage;
				if provider_thinking_level.is_some() {
					message.provider_thinking_level = provider_thinking_level;
				}
			}
			AssistantMessageEvent::Error {
				reason,
				error: partial.clone(),
			}
		}
	};
	Ok(Some(event))
}

/// `partial.content[contentIndex] = block`.
fn set_content(partial: &SharedAssistantMessage, content_index: usize, block: AssistantContent) {
	let content = &mut partial.borrow_mut().content;
	match content.get_mut(content_index) {
		Some(slot) => *slot = block,
		None => content.push(block),
	}
}
