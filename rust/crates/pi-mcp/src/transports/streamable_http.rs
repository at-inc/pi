//! Port of `packages/mcp/src/transports/streamable-http.ts`.
//! PORT-STATUS: ported
//!
//! `McpHttpError` and its subclasses are `PiError`s with `status` set and `data` holding `status`, `body` and,
//! for `McpAuthRequiredError`, `wwwAuthenticate`; `McpHttpError::is` also matches the subclasses, like
//! `instanceof`. Request headers start from the configured headers and are then set case-insensitively, like a
//! `Headers` object, but keep the caller's casing and order instead of the lowercased, sorted `Headers`
//! iteration. `consumeSseStream` splits lines on the raw bytes and decodes each line, which gives the same text
//! as a streaming `TextDecoder` because `\n` never occurs inside a UTF-8 sequence; its size limit counts the raw
//! bytes of the unterminated line. JSON parse errors carry serde_json's message instead of V8's.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use futures::StreamExt;
use futures::future::{Either, select};
use indexmap::IndexMap;
use pi_host::js::{json_stringify, utf16_len, utf16_prefix};
use pi_host::{AbortController, AbortSignal, Body, HttpRequest, HttpResponse, LocalBoxFuture, PiError, PiResult};
use serde::Serialize;
use serde_json::{Value, json};
use url::Url;

use super::transport::{
	DEFAULT_MAX_MESSAGE_BYTES, McpTransport, McpTransportCloseListener, McpTransportErrorListener,
	McpTransportMessageListener, McpTransportSendOptions, TransportEvents, Unsubscribe,
};
use crate::auth_provider::{AuthProvider, McpFetch, UnauthorizedContext};
use crate::protocol::jsonrpc::{
	JSON_RPC_ERROR_CODES, JsonRpcErrorObject, JsonRpcId, JsonRpcMessage, JsonRpcResponse, McpAbortError,
	McpConnectionClosedError, parse_json_rpc_message,
};

const MAX_ERROR_BODY_BYTES: usize = 8 * 1024;
const ERROR_MESSAGE_BODY_CHARS: usize = 500;
const DEFAULT_RECONNECT_INITIAL_DELAY_MS: f64 = 1_000.0;
const DEFAULT_RECONNECT_MAX_DELAY_MS: f64 = 30_000.0;
const DEFAULT_RECONNECT_MAX_RETRIES: f64 = 5.0;

#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct SseEvent {
	#[serde(skip_serializing_if = "Option::is_none")]
	pub event: Option<String>,
	pub data: String,
	#[serde(skip_serializing_if = "Option::is_none")]
	pub id: Option<String>,
}

/// `onId(id)`.
pub type SseIdListener<'a> = Box<dyn FnMut(&str) + 'a>;

#[derive(Default)]
pub struct ConsumeSseOptions<'a> {
	pub max_event_bytes: Option<usize>,
	pub on_event: Option<Box<dyn FnMut(SseEvent) + 'a>>,
	/// Called for every `id` field, including events without data (for example resumption priming events).
	pub on_id: Option<SseIdListener<'a>>,
	/// Called for every valid `retry` field, in milliseconds.
	pub on_retry: Option<Box<dyn FnMut(f64) + 'a>>,
}

struct SseParser<'a, 'b> {
	options: &'b mut ConsumeSseOptions<'a>,
	event_name: Option<String>,
	event_id: Option<String>,
	data_lines: Vec<String>,
	// Bytes of the pending event's data, including the "\n" joins, so events streamed as many
	// short `data:` lines without a terminating blank line cannot grow without bound.
	data_bytes: usize,
	max_event_bytes: usize,
}

impl SseParser<'_, '_> {
	fn dispatch(&mut self) {
		if self.data_lines.is_empty() {
			self.event_name = None;
			self.event_id = None;
			return;
		}
		let event = SseEvent {
			event: self.event_name.take().filter(|name| !name.is_empty()),
			data: std::mem::take(&mut self.data_lines).join("\n"),
			id: self.event_id.take().filter(|id| !id.is_empty()),
		};
		self.data_bytes = 0;
		if let Some(on_event) = &mut self.options.on_event {
			on_event(event);
		}
	}

	fn process_line(&mut self, raw_line: &str) -> PiResult<()> {
		let line = raw_line.strip_suffix('\r').unwrap_or(raw_line);
		if line.is_empty() {
			self.dispatch();
			return Ok(());
		}
		if line.starts_with(':') {
			return Ok(());
		}
		let (field, value) = match line.find(':') {
			None => (line, ""),
			Some(colon) => (&line[..colon], &line[colon + 1..]),
		};
		let value = value.strip_prefix(' ').unwrap_or(value);
		if field == "data" {
			self.data_bytes += value.len() + usize::from(!self.data_lines.is_empty());
			if self.data_bytes > self.max_event_bytes {
				return Err(event_too_large(self.max_event_bytes));
			}
			self.data_lines.push(value.to_string());
		} else if field == "event" {
			self.event_name = Some(value.to_string());
		} else if field == "id" && !value.contains('\0') {
			self.event_id = Some(value.to_string());
			if let Some(on_id) = &mut self.options.on_id {
				on_id(value);
			}
		} else if field == "retry"
			&& !value.is_empty()
			&& value.bytes().all(|byte| byte.is_ascii_digit())
			&& let Some(on_retry) = &mut self.options.on_retry
		{
			on_retry(value.parse::<f64>().unwrap_or(f64::INFINITY));
		}
		Ok(())
	}
}

fn event_too_large(max_event_bytes: usize) -> PiError {
	PiError::new(format!("MCP SSE event exceeds {max_event_bytes} bytes"))
}

pub async fn consume_sse_stream(mut stream: Body, mut options: ConsumeSseOptions<'_>) -> PiResult<()> {
	let max_event_bytes = options.max_event_bytes.unwrap_or(DEFAULT_MAX_MESSAGE_BYTES);
	let mut parser = SseParser {
		options: &mut options,
		event_name: None,
		event_id: None,
		data_lines: Vec::new(),
		data_bytes: 0,
		max_event_bytes,
	};
	let mut buffered: Vec<u8> = Vec::new();
	while let Some(chunk) = stream.next().await {
		buffered.extend_from_slice(&chunk?);
		while let Some(newline) = buffered.iter().position(|byte| *byte == b'\n') {
			let line: Vec<u8> = buffered.drain(..=newline).collect();
			parser.process_line(&String::from_utf8_lossy(&line[..newline]))?;
		}
		if buffered.len() > max_event_bytes {
			return Err(event_too_large(max_event_bytes));
		}
	}
	if !buffered.is_empty() {
		parser.process_line(&String::from_utf8_lossy(&buffered))?;
	}
	parser.dispatch();
	Ok(())
}

#[derive(Clone, Debug, Default)]
pub struct StreamableHttpReconnectOptions {
	/// Delay before the first reconnection attempt, unless the server sent a `retry` field. Default: 1000.
	pub initial_delay_ms: Option<f64>,
	/// Upper bound for the exponential backoff. Default: 30000.
	pub max_delay_ms: Option<f64>,
	/// Consecutive failed attempts before giving up on a stream. Default: 5.
	pub max_retries: Option<f64>,
}

#[derive(Clone, Default)]
pub struct StreamableHttpTransportOptions {
	pub url: String,
	pub headers: Option<IndexMap<String, String>>,
	pub fetch: Option<McpFetch>,
	/// Open the server-to-client GET stream after initialization. Default: true.
	pub open_get_stream: Option<bool>,
	pub max_message_bytes: Option<usize>,
	pub auth_provider: Option<AuthProvider>,
	/// Reconnection of dropped SSE streams (the GET stream, and response streams that carry event IDs).
	pub reconnect: Option<StreamableHttpReconnectOptions>,
}

/// `class McpHttpError extends Error { status; body }`.
pub struct McpHttpError;

impl McpHttpError {
	pub const NAME: &'static str = "McpHttpError";

	/// `new McpHttpError(status, message, body)`.
	#[allow(clippy::new_ret_no_self)]
	pub fn new(status: u16, message: impl Into<String>, body: impl Into<String>) -> PiError {
		Self::named(Self::NAME, status, message, body.into())
	}

	fn named(name: &str, status: u16, message: impl Into<String>, body: String) -> PiError {
		PiError::named(name, message)
			.with_status(status)
			.with_data(json!({ "status": status, "body": body }))
	}

	/// `error instanceof McpHttpError`, subclasses included.
	pub fn is(error: &PiError) -> bool {
		error.name == Self::NAME || McpAuthRequiredError::is(error) || McpSessionExpiredError::is(error)
	}

	/// `error.status`.
	pub fn status(error: &PiError) -> Option<u16> {
		error.status
	}

	/// `error.body`.
	pub fn body(error: &PiError) -> Option<String> {
		error.data.as_ref()?.get("body")?.as_str().map(str::to_string)
	}
}

/// `class McpAuthRequiredError extends McpHttpError { wwwAuthenticate }`.
pub struct McpAuthRequiredError;

impl McpAuthRequiredError {
	pub const NAME: &'static str = "McpAuthRequiredError";

	/// `new McpAuthRequiredError(response, body)`.
	#[allow(clippy::new_ret_no_self)]
	pub fn new(response: &HttpResponse, body: impl Into<String>) -> PiError {
		let mut error = McpHttpError::named(Self::NAME, 401, "MCP server requires authentication", body.into());
		if let Some(Value::Object(data)) = &mut error.data {
			data.insert("wwwAuthenticate".into(), json!(response.header("www-authenticate")));
		}
		error
	}

	/// `error instanceof McpAuthRequiredError`.
	pub fn is(error: &PiError) -> bool {
		error.name == Self::NAME
	}

	/// `error.wwwAuthenticate`.
	pub fn www_authenticate(error: &PiError) -> Option<String> {
		error
			.data
			.as_ref()?
			.get("wwwAuthenticate")?
			.as_str()
			.map(str::to_string)
	}
}

/// `class McpSessionExpiredError extends McpHttpError`.
pub struct McpSessionExpiredError;

impl McpSessionExpiredError {
	pub const NAME: &'static str = "McpSessionExpiredError";

	/// `new McpSessionExpiredError(body)`.
	#[allow(clippy::new_ret_no_self)]
	pub fn new(body: impl Into<String>) -> PiError {
		McpHttpError::named(Self::NAME, 404, "MCP session expired", body.into())
	}

	/// `error instanceof McpSessionExpiredError`.
	pub fn is(error: &PiError) -> bool {
		error.name == Self::NAME
	}
}

fn content_type(response: &HttpResponse) -> Option<String> {
	response
		.header("content-type")
		.map(|value| value.split(';').next().unwrap_or("").trim().to_lowercase())
}

/// 401, or 403 with an `insufficient_scope` bearer challenge (step-up authorization).
fn needs_authorization(response: &HttpResponse) -> bool {
	if response.status == 401 {
		return true;
	}
	if response.status != 403 {
		return false;
	}
	let pattern = regex::Regex::new(r#"(?i)(?:^|[\s,])error="?insufficient_scope"?"#).expect("valid pattern");
	pattern.is_match(response.header("www-authenticate").unwrap_or(""))
}

/// Statuses worth retrying when a stream fails to (re)open.
fn is_transient_status(status: u16) -> bool {
	status == 408 || status == 429 || status >= 500
}

fn describe_http_failure(status: u16, body: &str) -> String {
	let text = body.trim();
	let snippet = if utf16_len(text) > ERROR_MESSAGE_BODY_CHARS {
		format!("{}...", utf16_prefix(text, ERROR_MESSAGE_BODY_CHARS - 3))
	} else {
		text.to_string()
	};
	let suffix = if snippet.is_empty() {
		String::new()
	} else {
		format!(": {snippet}")
	};
	format!("MCP HTTP request failed with status {status}{suffix}")
}

#[derive(Default)]
struct StreamCursor {
	last_event_id: Option<String>,
	retry_ms: Option<f64>,
	/// Whether the stream delivered any event since it was (re)opened.
	received: bool,
}

struct Inner {
	events: TransportEvents,
	url: Url,
	options: StreamableHttpTransportOptions,
	fetch: McpFetch,
	controller: AbortController,
	started: Cell<bool>,
	closed: Cell<bool>,
	session_id: RefCell<Option<String>>,
	protocol_version: RefCell<Option<String>>,
	get_stream_started: Cell<bool>,
	/// Access token of the latest request, which `close()` reuses instead of asking the auth provider.
	last_token: RefCell<Option<String>>,
}

#[derive(Clone)]
pub struct StreamableHttpTransport {
	inner: Rc<Inner>,
}

impl StreamableHttpTransport {
	/// `new StreamableHttpTransport(options)`. Fails like `new URL(options.url)` for an invalid URL.
	pub fn new(options: StreamableHttpTransportOptions) -> PiResult<Self> {
		let url = Url::parse(&options.url).map_err(|_| PiError::type_error("Invalid URL"))?;
		let override_fetch = options.fetch.clone();
		let fetch: McpFetch = Rc::new(move |request| {
			let override_fetch = override_fetch.clone();
			Box::pin(async move { pi_host::fetch(request, override_fetch.as_ref()).await })
		});
		Ok(Self {
			inner: Rc::new(Inner {
				events: TransportEvents::new(),
				url,
				options,
				fetch,
				controller: AbortController::new(),
				started: Cell::new(false),
				closed: Cell::new(false),
				session_id: RefCell::new(None),
				protocol_version: RefCell::new(None),
				get_stream_started: Cell::new(false),
				last_token: RefCell::new(None),
			}),
		})
	}

	pub fn url(&self) -> &Url {
		&self.inner.url
	}

	pub fn options(&self) -> &StreamableHttpTransportOptions {
		&self.inner.options
	}

	pub fn session_id(&self) -> Option<String> {
		self.inner.session_id.borrow().clone()
	}
}

impl McpTransport for StreamableHttpTransport {
	fn start(&self) -> LocalBoxFuture<'static, PiResult<()>> {
		let inner = &self.inner;
		let result = if inner.started.get() {
			Err(PiError::new("MCP Streamable HTTP transport already started"))
		} else if inner.closed.get() {
			Err(McpConnectionClosedError::new())
		} else {
			inner.started.set(true);
			Ok(())
		};
		Box::pin(async move { result })
	}

	fn set_protocol_version(&self, version: &str) {
		*self.inner.protocol_version.borrow_mut() = Some(version.to_string());
	}

	fn send(&self, message: JsonRpcMessage, options: McpTransportSendOptions) -> LocalBoxFuture<'static, PiResult<()>> {
		let inner = self.inner.clone();
		Box::pin(async move { send(inner, message, options.signal).await })
	}

	fn close(&self) -> LocalBoxFuture<'static, PiResult<()>> {
		let inner = self.inner.clone();
		if inner.closed.get() {
			return Box::pin(async { Ok(()) });
		}
		inner.closed.set(true);
		inner.controller.abort();
		Box::pin(async move {
			let session_id = inner.session_id.borrow().clone();
			if inner.started.get() && session_id.as_deref().is_some_and(|id| !id.is_empty()) {
				// Best effort: the session expires on the server anyway. The auth provider may refresh tokens
				// over the network, so it is not asked here and closing never waits for a refresh.
				let token = inner.last_token.borrow().clone();
				let timeout = AbortController::new();
				let request = HttpRequest {
					method: "DELETE".into(),
					url: inner.url.to_string(),
					headers: build_headers(&inner, &[], token.as_deref()),
					body: None,
					signal: Some(timeout.signal()),
				};
				let response = (inner.fetch)(request);
				let deadline = pi_host::sleep(1_000.0, None);
				futures::pin_mut!(deadline);
				if let Either::Right(_) = select(response, deadline).await {
					timeout.abort_with(PiError::named(
						"TimeoutError",
						"The operation was aborted due to timeout",
					));
				}
			}
			inner.events.emit_close();
			Ok(())
		})
	}

	fn on_message(&self, listener: McpTransportMessageListener) -> Unsubscribe {
		self.inner.events.on_message(listener)
	}

	fn on_error(&self, listener: McpTransportErrorListener) -> Unsubscribe {
		self.inner.events.on_error(listener)
	}

	fn on_close(&self, listener: McpTransportCloseListener) -> Unsubscribe {
		self.inner.events.on_close(listener)
	}
}

async fn send(inner: Rc<Inner>, message: JsonRpcMessage, signal: Option<AbortSignal>) -> PiResult<()> {
	if !inner.started.get() || inner.closed.get() {
		return Err(McpConnectionClosedError::new());
	}
	let response = authorized_fetch(
		&inner,
		"POST",
		&[
			("accept", "application/json, text/event-stream"),
			("content-type", "application/json"),
		],
		Some(json_stringify(&message)),
		signal.as_ref(),
	)
	.await?;
	let response = check_response(&inner, response).await?;
	capture_session(&inner, &response);

	let JsonRpcMessage::Request(request) = &message else {
		// Notifications and responses are acknowledged with 202 and carry no reply; ignore any body.
		drop(response);
		// The server-to-client stream may only open once the session is initialized.
		if message.method() == Some("notifications/initialized") {
			start_get_stream(&inner);
		}
		return Ok(());
	};
	if response.status == 202 || response.status == 204 {
		return Err(McpHttpError::new(
			response.status,
			format!("MCP server accepted request {} without a response", request.method),
			"",
		));
	}
	let kind = content_type(&response);
	if kind.as_deref() == Some("application/json") {
		let body = response.json().await?;
		let items = match body {
			Value::Array(items) => items,
			body => vec![body],
		};
		for item in items {
			inner.events.emit_message(&parse_json_rpc_message(item)?);
		}
		return Ok(());
	}
	if kind.as_deref() == Some("text/event-stream") {
		let id = request.id.clone();
		let task_inner = inner.clone();
		pi_host::spawn_local(async move { consume_response_stream(task_inner, response.body, id).await });
		return Ok(());
	}
	let status = response.status;
	drop(response);
	Err(McpHttpError::new(
		status,
		format!(
			"Unsupported MCP response content type: {}",
			kind.as_deref().unwrap_or("missing")
		),
		"",
	))
}

/// Fetch with auth headers. A 401 (or a 403 asking for more scope) is handed to the auth provider
/// once, and the request is retried with whatever credentials it left behind.
///
/// `signal` only stops dispatch: getting a token or the authorization recovery can take a while, and a
/// request its sender gave up on in the meantime must not reach the server. It does not abort the shared
/// recovery, which other requests may wait for.
async fn authorized_fetch(
	inner: &Rc<Inner>,
	method: &str,
	extra: &[(&str, &str)],
	body: Option<String>,
	signal: Option<&AbortSignal>,
) -> PiResult<HttpResponse> {
	let on_unauthorized = inner
		.options
		.auth_provider
		.as_ref()
		.and_then(|provider| provider.on_unauthorized.clone());
	let mut attempt = 0;
	loop {
		let token = token(inner).await?;
		if signal.is_some_and(AbortSignal::aborted) {
			return Err(McpAbortError::new());
		}
		let response = (inner.fetch)(HttpRequest {
			method: method.to_string(),
			url: inner.url.to_string(),
			headers: build_headers(inner, extra, token.as_deref()),
			body: body.clone().map(Into::into),
			signal: Some(inner.controller.signal()),
		})
		.await?;
		let on_unauthorized = match &on_unauthorized {
			Some(on_unauthorized) if attempt == 0 && needs_authorization(&response) => on_unauthorized,
			_ => return Ok(response),
		};
		on_unauthorized(UnauthorizedContext {
			response,
			server_url: inner.url.clone(),
			fetch: inner.fetch.clone(),
			token: token.filter(|token| !token.is_empty()),
		})
		.await?;
		attempt += 1;
	}
}

async fn token(inner: &Rc<Inner>) -> PiResult<Option<String>> {
	let token = match &inner.options.auth_provider {
		Some(provider) => (provider.token)().await?,
		None => None,
	};
	*inner.last_token.borrow_mut() = token.clone();
	Ok(token)
}

/// `headers.set(name, value)` on a header map that keeps the caller's casing.
pub(crate) fn set_header(headers: &mut IndexMap<String, String>, name: &str, value: &str) {
	match headers.keys().position(|key| key.eq_ignore_ascii_case(name)) {
		Some(index) => {
			headers.shift_remove_index(index);
			headers.shift_insert(index, name.to_string(), value.to_string());
		}
		None => {
			headers.insert(name.to_string(), value.to_string());
		}
	}
}

fn build_headers(inner: &Inner, extra: &[(&str, &str)], token: Option<&str>) -> IndexMap<String, String> {
	let mut headers: IndexMap<String, String> = IndexMap::new();
	for (name, value) in inner.options.headers.iter().flatten() {
		set_header(&mut headers, name, value);
	}
	for (name, value) in extra {
		set_header(&mut headers, name, value);
	}
	if let Some(session_id) = inner.session_id.borrow().as_deref().filter(|id| !id.is_empty()) {
		set_header(&mut headers, "Mcp-Session-Id", session_id);
	}
	if let Some(version) = inner
		.protocol_version
		.borrow()
		.as_deref()
		.filter(|version| !version.is_empty())
	{
		set_header(&mut headers, "MCP-Protocol-Version", version);
	}
	if let Some(token) = token.filter(|token| !token.is_empty()) {
		set_header(&mut headers, "Authorization", &format!("Bearer {token}"));
	}
	headers
}

fn capture_session(inner: &Inner, response: &HttpResponse) {
	if let Some(session_id) = response.header("mcp-session-id").filter(|id| !id.is_empty()) {
		*inner.session_id.borrow_mut() = Some(session_id.to_string());
	}
}

async fn check_response(inner: &Inner, response: HttpResponse) -> PiResult<HttpResponse> {
	if response.ok() {
		return Ok(response);
	}
	let status = response.status;
	let auth_required = (status == 401).then(|| McpAuthRequiredError::new(&response, ""));
	let text = response.text().await.unwrap_or_default();
	let body = utf16_prefix(&text, MAX_ERROR_BODY_BYTES).to_string();
	if let Some(mut error) = auth_required {
		if let Some(Value::Object(data)) = &mut error.data {
			data.insert("body".into(), json!(body));
		}
		return Err(error);
	}
	if status == 404 && inner.session_id.borrow().as_deref().is_some_and(|id| !id.is_empty()) {
		return Err(McpSessionExpiredError::new(body));
	}
	Err(McpHttpError::new(status, describe_http_failure(status, &body), body))
}

async fn consume_sse(
	inner: &Rc<Inner>,
	stream: Body,
	cursor: &RefCell<StreamCursor>,
	on_message: Option<&dyn Fn(&JsonRpcMessage)>,
) -> PiResult<()> {
	consume_sse_stream(
		stream,
		ConsumeSseOptions {
			max_event_bytes: Some(inner.options.max_message_bytes.unwrap_or(DEFAULT_MAX_MESSAGE_BYTES)),
			on_id: Some(Box::new(|id| cursor.borrow_mut().last_event_id = Some(id.to_string()))),
			on_retry: Some(Box::new(|delay_ms| cursor.borrow_mut().retry_ms = Some(delay_ms))),
			on_event: Some(Box::new(|event| {
				cursor.borrow_mut().received = true;
				// Events without data prime resumption; other event types are not JSON-RPC.
				if event.data.trim().is_empty() || event.event.as_deref().is_some_and(|name| name != "message") {
					return;
				}
				let message = serde_json::from_str::<Value>(&event.data)
					.map_err(PiError::from)
					.and_then(parse_json_rpc_message);
				match message {
					Err(error) => inner.events.emit_error(&error),
					Ok(message) => {
						if let Some(on_message) = on_message {
							on_message(&message);
						}
						inner.events.emit_message(&message);
					}
				}
			})),
		},
	)
	.await
}

/// Read the SSE stream answering one request. When the stream ends or breaks before the response
/// arrives and the server assigned event IDs, resume it with GET and `Last-Event-ID`, as the
/// server may close response streams at will. Otherwise only this request fails.
async fn consume_response_stream(inner: Rc<Inner>, body: Body, request_id: JsonRpcId) {
	let cursor = RefCell::new(StreamCursor::default());
	let answered = Cell::new(false);
	let on_message = |message: &JsonRpcMessage| {
		if let JsonRpcMessage::Response(response) = message
			&& response_id(response) == &request_id
		{
			answered.set(true);
		}
	};
	let mut stream = Some(body);
	let mut failure: Option<PiError> = None;
	let mut attempt = 0.0;
	loop {
		if let Some(body) = stream.take() {
			failure = consume_sse(&inner, body, &cursor, Some(&on_message)).await.err();
		}
		if answered.get() || inner.closed.get() {
			return;
		}
		if failure.as_ref().is_some_and(|failure| !is_retryable(failure)) {
			break;
		}
		if cursor.borrow().last_event_id.is_none() || attempt >= max_retries(&inner) {
			break;
		}
		if cursor.borrow().received {
			attempt = 0.0;
		}
		cursor.borrow_mut().received = false;
		let retry_ms = cursor.borrow().retry_ms;
		let delay = reconnect_delay(&inner, attempt, retry_ms);
		attempt += 1.0;
		if !sleep(&inner, delay).await {
			return;
		}
		let last_event_id = cursor.borrow().last_event_id.clone();
		match open_sse_stream(&inner, last_event_id).await {
			Ok(body) => stream = body,
			Err(error) => {
				let retryable = is_retryable(&error);
				failure = Some(error);
				if !retryable {
					break;
				}
			}
		}
	}
	if inner.closed.get() {
		return;
	}
	let reason = match &failure {
		None => "stream ended without a response".to_string(),
		Some(failure) => failure.message.clone(),
	};
	inner.events.emit_message(&JsonRpcMessage::error(
		request_id,
		JsonRpcErrorObject {
			code: JSON_RPC_ERROR_CODES::INTERNAL_ERROR,
			message: format!("MCP response stream failed: {reason}"),
			data: None,
		},
	));
}

fn response_id(response: &JsonRpcResponse) -> &JsonRpcId {
	response.id()
}

fn start_get_stream(inner: &Rc<Inner>) {
	if inner.options.open_get_stream == Some(false) || inner.get_stream_started.get() || inner.closed.get() {
		return;
	}
	inner.get_stream_started.set(true);
	let inner = inner.clone();
	pi_host::spawn_local(async move { run_get_stream(inner).await });
}

/// Keep the server-to-client stream open, reconnecting with backoff when it drops.
async fn run_get_stream(inner: Rc<Inner>) {
	let cursor = RefCell::new(StreamCursor::default());
	let mut attempt = 0.0;
	while !inner.closed.get() {
		let last_event_id = cursor.borrow().last_event_id.clone();
		let outcome: PiResult<bool> = async {
			let Some(stream) = open_sse_stream(&inner, last_event_id).await? else {
				// The server does not offer a GET stream.
				return Ok(false);
			};
			let opened_at = pi_host::now_ms();
			consume_sse(&inner, stream, &cursor, None).await?;
			// A stream that stayed up for a while counts as healthy, even if it was idle.
			if cursor.borrow().received || pi_host::now_ms() - opened_at > max_delay(&inner) {
				attempt = 0.0;
			}
			Ok(true)
		}
		.await;
		match outcome {
			Ok(false) => return,
			Ok(true) => {}
			Err(error) => {
				if inner.closed.get() {
					return;
				}
				if !is_retryable(&error) {
					inner.events.emit_error(&error);
					return;
				}
			}
		}
		cursor.borrow_mut().received = false;
		if attempt >= max_retries(&inner) {
			inner.events.emit_error(&PiError::new(
				"MCP server-to-client stream dropped and could not be reopened",
			));
			return;
		}
		let retry_ms = cursor.borrow().retry_ms;
		let delay = reconnect_delay(&inner, attempt, retry_ms);
		attempt += 1.0;
		if !sleep(&inner, delay).await {
			return;
		}
	}
}

/// Open a GET SSE stream. Resolves to `None` when the server answers 405 (no GET stream).
async fn open_sse_stream(inner: &Rc<Inner>, last_event_id: Option<String>) -> PiResult<Option<Body>> {
	let mut extra = vec![("accept", "text/event-stream")];
	if let Some(last_event_id) = &last_event_id {
		extra.push(("last-event-id", last_event_id));
	}
	let response = authorized_fetch(inner, "GET", &extra, None, None).await?;
	if response.status == 405 {
		return Ok(None);
	}
	let response = check_response(inner, response).await?;
	capture_session(inner, &response);
	let kind = content_type(&response);
	if kind.as_deref() != Some("text/event-stream") {
		return Err(McpHttpError::new(
			response.status,
			format!(
				"Unsupported MCP GET response content type: {}",
				kind.as_deref().unwrap_or("missing")
			),
			"",
		));
	}
	Ok(Some(response.body))
}

/// Network failures and transient statuses are retried; auth, session, and protocol errors are not.
/// `fetch` reports network failures, including a connection dropped mid-body, as `TypeError`.
fn is_retryable(error: &PiError) -> bool {
	if McpHttpError::is(error) {
		return is_transient_status(error.status.unwrap_or(0));
	}
	if error.name == "TypeError" {
		return true;
	}
	error
		.data
		.as_ref()
		.and_then(|data| data.get("code"))
		.and_then(Value::as_str)
		.is_some_and(|code| code.starts_with('E') || code.starts_with("UND_ERR"))
}

fn reconnect_delay(inner: &Inner, attempt: f64, server_delay_ms: Option<f64>) -> f64 {
	if let Some(server_delay_ms) = server_delay_ms {
		return server_delay_ms;
	}
	let initial = inner
		.options
		.reconnect
		.as_ref()
		.and_then(|reconnect| reconnect.initial_delay_ms)
		.unwrap_or(DEFAULT_RECONNECT_INITIAL_DELAY_MS);
	(initial * 2f64.powf(attempt)).min(max_delay(inner))
}

fn max_delay(inner: &Inner) -> f64 {
	inner
		.options
		.reconnect
		.as_ref()
		.and_then(|reconnect| reconnect.max_delay_ms)
		.unwrap_or(DEFAULT_RECONNECT_MAX_DELAY_MS)
}

fn max_retries(inner: &Inner) -> f64 {
	inner
		.options
		.reconnect
		.as_ref()
		.and_then(|reconnect| reconnect.max_retries)
		.unwrap_or(DEFAULT_RECONNECT_MAX_RETRIES)
}

/// Resolves `false` when the transport closed while waiting.
async fn sleep(inner: &Inner, ms: f64) -> bool {
	let signal = inner.controller.signal();
	if signal.aborted() {
		return false;
	}
	pi_host::sleep(ms, Some(&signal)).await.is_ok()
}
