//! Port of `packages/ai/src/api/anthropic-messages.ts`.
//! PORT-STATUS: ported
//!
//! Differences from the TS:
//! - The SDK client is [`AnthropicClient`] from `api/anthropic_client.rs`. `AnthropicOptions.client` takes any
//!   [`AnthropicClientApi`], the part of the SDK's `Anthropic` type this file calls, so callers can inject another
//!   client the way the TS accepts an `AnthropicVertex` or a test fake.
//! - `AnthropicEffort` and `AnthropicThinkingDisplay` are open strings: a model's `thinkingLevelMap` can map a level to
//!   any effort string, which the TS sends unchanged. `toolChoice` is the JSON value the TS accepts (a choice string or
//!   a `{ type: "tool", name }` object).
//! - `ProviderStreams.stream` reads the Anthropic option keys from `ProviderStreamOptions.extra`; a key whose value has
//!   another JSON type than the option's is ignored, where the TS would test that value for truthiness.
//! - The TS `stream` runs its async body synchronously up to the first `await` (the `onPayload` call); here the whole
//!   body runs on the host executor after `stream` returns.
//! - The federation client cache is per thread, like every Rust module-level state of the port.

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, VecDeque};
use std::ops::{Deref, DerefMut};
use std::rc::Rc;

use futures::StreamExt;
use indexmap::{IndexMap, IndexSet};
use pi_host::js::{json_stringify, math_max, math_min};
use pi_host::{AbortSignal, Body, FetchFunction, HttpResponse, PiError, PiResult};
use serde_json::{Map, Value, json};

use crate::api::anthropic_client::{
	AnthropicClient, AnthropicClientApi, AnthropicConfig, ClientOptions, IdentityTokenConfig,
	OidcFederationAuthentication, RequestOptions, template_string, truthy,
};
use crate::api::constrained_sampling::{
	UnsupportedStrictSchemaKeywordCheck, get_json_schema_tool_parameters, is_strict_tool_limit_error_message,
	prefers_json_schema_strict_sampling, resolve_json_schema_strict_sampling,
};
use crate::api::github_copilot_headers::{build_copilot_dynamic_headers, has_copilot_vision_input};
use crate::api::simple_options::{adjust_max_tokens_for_thinking, build_base_options, clamp_max_tokens_to_context};
use crate::api::transform_messages::{split_error_tool_result_images, transform_messages};
use crate::env_api_keys::{
	ANTHROPIC_FEDERATION_RULE_ID_ENV, ANTHROPIC_IDENTITY_TOKEN_FILE_ENV, ANTHROPIC_ORGANIZATION_ID_ENV,
	ANTHROPIC_SERVICE_ACCOUNT_ID_ENV, ANTHROPIC_WORKSPACE_ID_ENV,
};
use crate::models::calculate_cost;
use crate::providers::compat_schema::{AnthropicMessagesCompat, AnthropicSessionAffinityFormat};
use crate::types::{
	AssistantContent, AssistantMessage, AssistantMessageEvent, CacheRetention, ImageContent, Message, Model,
	ModelThinkingLevel, ProviderEnv, ProviderHeaders, ProviderResponse, ProviderStreamOptions, ProviderStreams,
	SharedAssistantMessage, SimpleStreamOptions, StopReason, StreamOptions, TextContent, ThinkingContent,
	ThinkingLevel, Tool, ToolCall, ToolChoice, ToolResultMessage, TranscriptContext, UserContent, UserMessageContent,
};
use crate::utils::diagnostics::{
	AssistantMessageDiagnostic, DiagnosticErrorInfo, append_assistant_message_diagnostic,
	create_provider_error_diagnostic,
};
use crate::utils::event_stream::AssistantMessageEventStream;
use crate::utils::headers::headers_to_record;
use crate::utils::identity::{ClientIdentity, ClientIdentityTarget, get_identity_headers};
use crate::utils::json_parse::{number_value, parse_json_with_repair};
use crate::utils::openrouter_cost::{apply_open_router_reported_cost, is_open_router_model};
use crate::utils::provider_env::get_provider_env_value;
use crate::utils::provider_retry::{ProviderRetryOptions, retry_provider_request};
use crate::utils::sanitize_unicode::sanitize_surrogates;
use crate::utils::streaming_json::ToolCallArgumentsStream;
use crate::utils::text::{get_system_message_text, js_trim, render_system_message_update};
use crate::utils::transcript::{get_current_tools, get_initial_system_message, resolve_transcript};

/// Resolve cache retention preference.
/// Defaults to "short" and uses PI_CACHE_RETENTION for backward compatibility.
fn resolve_cache_retention(cache_retention: Option<CacheRetention>, env: Option<&ProviderEnv>) -> CacheRetention {
	if let Some(cache_retention) = cache_retention {
		return cache_retention;
	}
	if get_provider_env_value("PI_CACHE_RETENTION", env).as_deref() == Some("long") {
		return CacheRetention::Long;
	}
	CacheRetention::Short
}

/// `getCacheControl(...).cacheControl`.
fn get_cache_control(
	model: &Model,
	cache_retention: Option<CacheRetention>,
	env: Option<&ProviderEnv>,
) -> Option<Value> {
	let retention = resolve_cache_retention(cache_retention, env);
	if retention == CacheRetention::None {
		return None;
	}
	let mut cache_control = Map::new();
	cache_control.insert("type".into(), "ephemeral".into());
	if retention == CacheRetention::Long && get_anthropic_compat(model).supports_long_cache_retention {
		cache_control.insert("ttl".into(), "1h".into());
	}
	Some(Value::Object(cache_control))
}

// Stealth mode: Mimic Claude Code's tool naming exactly
const CLAUDE_CODE_VERSION: &str = "2.1.307";

// Claude Code 2.x tool names (canonical casing)
// Source: https://cchistory.mariozechner.at/data/prompts-2.1.11.md
// To update: https://github.com/badlogic/cchistory
const CLAUDE_CODE_TOOLS: [&str; 17] = [
	"Read",
	"Write",
	"Edit",
	"Bash",
	"Grep",
	"Glob",
	"AskUserQuestion",
	"EnterPlanMode",
	"ExitPlanMode",
	"KillShell",
	"NotebookEdit",
	"Skill",
	"Task",
	"TaskOutput",
	"TodoWrite",
	"WebFetch",
	"WebSearch",
];

// Convert tool name to CC canonical casing if it matches (case-insensitive)
fn to_claude_code_name(name: &str) -> String {
	let lower_name = name.to_lowercase();
	CLAUDE_CODE_TOOLS
		.iter()
		.find(|tool| tool.to_lowercase() == lower_name)
		.map_or_else(|| name.to_string(), |tool| tool.to_string())
}

fn from_claude_code_name(name: &str, tools: &[Tool]) -> String {
	let lower_name = name.to_lowercase();
	tools
		.iter()
		.find(|tool| tool.name.to_lowercase() == lower_name)
		.map_or_else(|| name.to_string(), |tool| tool.name.clone())
}

fn convert_image_block(image: &ImageContent) -> Value {
	json!({
		"type": "image",
		"source": {
			"type": "base64",
			"media_type": image.mime_type,
			"data": image.data,
		},
	})
}

/// Convert content blocks to Anthropic API format
fn convert_content_blocks(content: &[UserContent]) -> Value {
	// If only text blocks, return as concatenated string for simplicity
	let has_images = content.iter().any(|block| matches!(block, UserContent::Image(_)));
	if !has_images {
		let text: Vec<&str> = content
			.iter()
			.map(|block| match block {
				UserContent::Text(block) => block.text.as_str(),
				UserContent::Image(_) => unreachable!("content has no images"),
			})
			.collect();
		return Value::String(sanitize_surrogates(&text.join("\n")).to_string());
	}

	// If we have images, convert to content block array
	let mut blocks: Vec<Value> = content
		.iter()
		.map(|block| match block {
			UserContent::Text(block) => json!({ "type": "text", "text": sanitize_surrogates(&block.text) }),
			UserContent::Image(image) => convert_image_block(image),
		})
		.collect();

	// If only images (no text), add placeholder text block
	let has_text = content.iter().any(|block| matches!(block, UserContent::Text(_)));
	if !has_text {
		blocks.insert(0, json!({ "type": "text", "text": "(see attached image)" }));
	}

	Value::Array(blocks)
}

/// `AnthropicEffort`: `"low" | "medium" | "high" | "xhigh" | "max"` (see the file header).
pub type AnthropicEffort = String;

/// `AnthropicThinkingDisplay`: `"summarized" | "omitted"` (see the file header).
pub type AnthropicThinkingDisplay = String;

const FINE_GRAINED_TOOL_STREAMING_BETA: &str = "fine-grained-tool-streaming-2025-05-14";
const INTERLEAVED_THINKING_BETA: &str = "interleaved-thinking-2025-05-14";
const SERVER_SIDE_FALLBACK_BETA: &str = "server-side-fallback-2026-07-01";
const MID_CONVERSATION_OUTPUT_CONFIG_BETA: &str = "mid-conversation-output-config-2026-07-01";
const THINKING_BINDING_CONTROLS_BETA: &str = "thinking-binding-controls-2026-08-01";
const INLINE_TOOLS_BETA: &str = "inline-tools-2026-09-15";

/// Stable deferred tool declared whenever native tool changes are in use. Anthropic adds
/// hidden prompt scaffolding for mid-conversation tool changes; declaring this placeholder
/// from the first request keeps that scaffolding in the cached prefix, so the first tool
/// change does not invalidate the cache (measured: full miss without it). It is never
/// activated and the model cannot see it.
fn deferred_tool_placeholder() -> Value {
	json!({
		"name": "__pi_deferred_placeholder__",
		"description": "Reserved placeholder. Never available. Never call this.",
		"input_schema": { "type": "object", "properties": {}, "required": [] },
		"defer_loading": true,
	})
}

fn should_use_server_side_fallback_beta(compat: &AnthropicMessagesCompat) -> bool {
	compat
		.allowed_fallback_models
		.as_ref()
		.is_some_and(|models| !models.is_empty())
}

/// `getAnthropicCompat(model)`.
struct AnthropicCompat {
	supports_eager_tool_input_streaming: bool,
	supports_long_cache_retention: bool,
	send_session_affinity_headers: bool,
	session_affinity_format: Option<AnthropicSessionAffinityFormat>,
	supports_cache_control_on_tools: bool,
	supports_temperature: bool,
	allow_empty_signature: bool,
	supports_strict_tools: bool,
	supports_mid_convo_system_messages: bool,
	supports_mid_convo_tool_changes: bool,
}

fn get_anthropic_compat(model: &Model) -> AnthropicCompat {
	let compat = model.anthropic_messages_compat();
	let is_open_router = model.provider == "openrouter" || model.base_url.contains("openrouter.ai");
	AnthropicCompat {
		supports_eager_tool_input_streaming: compat.supports_eager_tool_input_streaming.unwrap_or(true),
		supports_long_cache_retention: compat.supports_long_cache_retention.unwrap_or(true),
		send_session_affinity_headers: compat.send_session_affinity_headers.unwrap_or(is_open_router),
		session_affinity_format: compat
			.session_affinity_format
			.or(is_open_router.then_some(AnthropicSessionAffinityFormat::Openrouter)),
		supports_cache_control_on_tools: compat.supports_cache_control_on_tools.unwrap_or(true),
		supports_temperature: compat.supports_temperature.unwrap_or(true),
		allow_empty_signature: compat.allow_empty_signature.unwrap_or(false),
		supports_strict_tools: compat.supports_strict_tools.unwrap_or(false),
		supports_mid_convo_system_messages: compat.supports_mid_convo_system_messages.unwrap_or(false),
		supports_mid_convo_tool_changes: compat.supports_mid_convo_tool_changes.unwrap_or(false),
	}
}

/// `AnthropicOptions`.
#[derive(Clone, Default)]
pub struct AnthropicOptions {
	pub base: StreamOptions,
	/// Enable extended thinking.
	/// For adaptive thinking models: the model decides when/how much to think.
	/// For older models: uses budget-based thinking with thinkingBudgetTokens.
	/// Default: undefined (thinking is omitted unless `streamSimple()` maps
	/// a simple reasoning level to this option, or callers set it explicitly).
	pub thinking_enabled: Option<bool>,
	/// Token budget for extended thinking (older models only).
	/// Ignored for adaptive thinking models.
	/// Default: 1024 when `thinkingEnabled` is true and no budget is provided.
	pub thinking_budget_tokens: Option<f64>,
	/// Effort level for adaptive thinking models.
	/// Controls how much thinking Claude allocates:
	/// - "max": Always thinks with no constraints (Opus 4.6 only)
	/// - "xhigh": Highest reasoning level (Opus 4.7+, Fable 5)
	/// - "high": Always thinks, deep reasoning
	/// - "medium": Moderate thinking, may skip for simple queries
	/// - "low": Minimal thinking, skips for simple tasks
	///
	/// Ignored for older models.
	/// Default: omitted unless `streamSimple()` maps a simple reasoning
	/// level to this option.
	pub effort: Option<AnthropicEffort>,
	/// Controls how thinking content is returned in API responses.
	/// - "summarized": Thinking blocks contain summarized thinking text.
	/// - "omitted": Thinking blocks return an empty thinking field; the encrypted
	///   signature still travels back for multi-turn continuity. Use for faster
	///   time-to-first-text-token when your UI does not surface thinking.
	///
	/// Note: Anthropic's API default for Claude Opus 4.7 and Claude Mythos Preview
	/// is "omitted". We default to "summarized" here to keep behavior consistent
	/// with older Claude 4 models. Set this explicitly to "omitted" to opt in.
	/// Default: "summarized" when thinking is enabled.
	pub thinking_display: Option<AnthropicThinkingDisplay>,
	/// Whether to request the interleaved thinking beta header for non-adaptive
	/// thinking models. Adaptive thinking models have interleaved thinking built in,
	/// so the header is skipped for them regardless of this setting.
	/// Default: true.
	pub interleaved_thinking: Option<bool>,
	/// Anthropic tool choice behavior. String values map to Anthropic's built-in
	/// choices; `{ type: "tool", name }` forces a specific tool.
	/// Default: omitted (Anthropic default behavior, currently equivalent to auto).
	pub tool_choice: Option<Value>,
	/// Pre-built Anthropic client instance. When provided, skips internal client
	/// construction entirely. Use this to inject alternative SDK clients such as
	/// `AnthropicVertex` that shares the same messaging API.
	pub client: Option<Rc<dyn AnthropicClientApi>>,
}

impl Deref for AnthropicOptions {
	type Target = StreamOptions;

	fn deref(&self) -> &Self::Target {
		&self.base
	}
}

impl DerefMut for AnthropicOptions {
	fn deref_mut(&mut self) -> &mut Self::Target {
		&mut self.base
	}
}

impl AnthropicOptions {
	/// Reads the Anthropic option keys of `ProviderStreamOptions.extra` (see the file header).
	pub fn from_provider_options(options: ProviderStreamOptions) -> Self {
		let extra = &options.extra;
		let boolean = |key: &str| extra.get(key).and_then(Value::as_bool);
		let string = |key: &str| extra.get(key).and_then(Value::as_str).map(str::to_string);
		Self {
			thinking_enabled: boolean("thinkingEnabled"),
			thinking_budget_tokens: extra.get("thinkingBudgetTokens").and_then(Value::as_f64),
			effort: string("effort"),
			thinking_display: string("thinkingDisplay"),
			interleaved_thinking: boolean("interleavedThinking"),
			tool_choice: extra.get("toolChoice").filter(|choice| !choice.is_null()).cloned(),
			client: None,
			base: options.base,
		}
	}
}

/// `mergeHeaders(...headerSources)`: a later header replaces an earlier one of the same name in any case, and moves
/// to the end with its own casing.
fn merge_headers(header_sources: &[Option<&ProviderHeaders>]) -> ProviderHeaders {
	let mut merged = ProviderHeaders::new();
	for headers in header_sources.iter().flatten() {
		for (name, value) in headers.iter() {
			let lower_name = name.to_lowercase();
			if let Some(existing) = merged.keys().find(|key| key.to_lowercase() == lower_name).cloned() {
				merged.shift_remove(&existing);
			}
			merged.insert(name.clone(), value.clone());
		}
	}
	merged
}

/// A string header record as `ProviderHeaders`.
fn provider_headers(headers: &IndexMap<String, String>) -> ProviderHeaders {
	headers
		.iter()
		.map(|(name, value)| (name.clone(), Some(value.clone())))
		.collect()
}

/// `getAnthropicApiVersion(model, options)`: `None` is `undefined`, `Some(None)` is `null`.
fn get_anthropic_api_version(model: &Model, options: &AnthropicOptions) -> Option<Option<String>> {
	let model_headers = model.headers.as_ref().map(provider_headers);
	let headers = merge_headers(&[model_headers.as_ref(), options.headers.as_ref()]);
	for (name, value) in &headers {
		if name.to_lowercase() == "anthropic-version" {
			return Some(value.clone());
		}
	}
	if options.client.is_some() {
		return None;
	}
	Some(Some(
		if model.provider == "anthropic" {
			"2026-10-21"
		} else {
			"2023-06-01"
		}
		.to_string(),
	))
}

fn has_header(headers: Option<&ProviderHeaders>, name: &str) -> bool {
	let Some(headers) = headers else {
		return false;
	};
	let expected = name.to_lowercase();
	headers.iter().any(|(key, value)| {
		key.to_lowercase() == expected && value.as_deref().is_some_and(|value| !js_trim(value).is_empty())
	})
}

fn has_request_auth(api_key: Option<&str>, headers: Option<&ProviderHeaders>) -> bool {
	api_key.is_some_and(|api_key| !api_key.is_empty())
		|| has_header(headers, "authorization")
		|| has_header(headers, "x-api-key")
		|| has_header(headers, "cf-aig-authorization")
}

fn assert_request_auth(provider: &str, api_key: Option<&str>, headers: Option<&ProviderHeaders>) -> PiResult<()> {
	if !has_request_auth(api_key, headers) {
		return Err(PiError::new(format!("No API key for provider: {provider}")));
	}
	Ok(())
}

/// Workload identity federation config from the ANTHROPIC_* variables the
/// Anthropic SDK documents; the SDK performs the token exchange and refresh.
/// Only for the anthropic provider, since the exchange is an Anthropic API
/// endpoint, and only when no key or auth header was resolved.
fn get_anthropic_federation(
	model: &Model,
	api_key: Option<&str>,
	headers: Option<&ProviderHeaders>,
	env: Option<&ProviderEnv>,
) -> Option<AnthropicConfig> {
	if model.provider != "anthropic" || has_request_auth(api_key, headers) {
		return None;
	}
	let federation_rule_id =
		get_provider_env_value(ANTHROPIC_FEDERATION_RULE_ID_ENV, env).filter(|id| !id.is_empty())?;
	let organization_id = get_provider_env_value(ANTHROPIC_ORGANIZATION_ID_ENV, env).filter(|id| !id.is_empty())?;
	let identity_token_file =
		get_provider_env_value(ANTHROPIC_IDENTITY_TOKEN_FILE_ENV, env).filter(|path| !path.is_empty())?;
	Some(AnthropicConfig {
		organization_id: Some(organization_id),
		workspace_id: get_provider_env_value(ANTHROPIC_WORKSPACE_ID_ENV, env),
		authentication: OidcFederationAuthentication {
			r#type: "oidc_federation".into(),
			federation_rule_id: Some(federation_rule_id),
			service_account_id: get_provider_env_value(ANTHROPIC_SERVICE_ACCOUNT_ID_ENV, env),
			identity_token: Some(IdentityTokenConfig {
				source: "file".into(),
				path: Some(identity_token_file),
			}),
		},
	})
}

/// The cached federation client: its key, the fetch it was built with, and the client.
struct FederationClient {
	key: String,
	fetch: Option<FetchFunction>,
	client: Rc<AnthropicClient>,
}

thread_local! {
	/// The SDK caches the federated access token per client, but pi creates a client
	/// per request. Keep one client for the current federation config and fetch, and
	/// clone it per request with `withOptions()`, which shares the token cache.
	static FEDERATION_CLIENT: RefCell<Option<FederationClient>> = const { RefCell::new(None) };
}

/// `ServerSentEvent`.
struct ServerSentEvent {
	event: Option<String>,
	data: String,
	raw: Vec<String>,
}

/// `SseDecoderState`.
#[derive(Default)]
struct SseDecoderState {
	event: Option<String>,
	data: Vec<String>,
	raw: Vec<String>,
}

const ANTHROPIC_MESSAGE_EVENTS: [&str; 6] = [
	"message_start",
	"message_delta",
	"message_stop",
	"content_block_start",
	"content_block_delta",
	"content_block_stop",
];

fn flush_sse_event(state: &mut SseDecoderState) -> Option<ServerSentEvent> {
	if state.event.as_deref().is_none_or(str::is_empty) && state.data.is_empty() {
		return None;
	}

	let event = ServerSentEvent {
		event: state.event.take(),
		data: state.data.join("\n"),
		raw: std::mem::take(&mut state.raw),
	};
	state.data.clear();
	Some(event)
}

fn decode_sse_line(line: &str, state: &mut SseDecoderState) -> Option<ServerSentEvent> {
	if line.is_empty() {
		return flush_sse_event(state);
	}

	state.raw.push(line.to_string());
	if line.starts_with(':') {
		return None;
	}

	let (field_name, value) = match line.find(':') {
		None => (line, ""),
		Some(index) => (&line[..index], &line[index + 1..]),
	};
	let value = value.strip_prefix(' ').unwrap_or(value);

	if field_name == "event" {
		state.event = Some(value.to_string());
	} else if field_name == "data" {
		state.data.push(value.to_string());
	}

	None
}

/// `consumeLine(text)`: the first line of `text` and the byte index where the rest starts. A `\r` at the end of
/// `text` ends a line on its own, even when the next chunk starts with `\n`.
fn consume_line(text: &str) -> Option<(&str, usize)> {
	let line_break_index = text.find(['\r', '\n'])?;
	let mut next_index = line_break_index + 1;
	if text.as_bytes()[line_break_index] == b'\r' && text.as_bytes().get(next_index) == Some(&b'\n') {
		next_index += 1;
	}
	Some((&text[..line_break_index], next_index))
}

/// `TextDecoder` in streaming mode: lossy UTF-8 with an incomplete trailing sequence kept for the next chunk, and a
/// leading BOM removed.
#[derive(Default)]
struct StreamingTextDecoder {
	pending: Vec<u8>,
	started: bool,
}

impl StreamingTextDecoder {
	/// `decoder.decode(chunk, { stream })`; `stream: false` flushes an incomplete sequence as U+FFFD.
	fn decode(&mut self, chunk: &[u8], stream: bool) -> String {
		self.pending.extend_from_slice(chunk);
		let mut out = String::new();
		let mut position = 0;
		while position < self.pending.len() {
			match std::str::from_utf8(&self.pending[position..]) {
				Ok(text) => {
					out.push_str(text);
					position = self.pending.len();
				}
				Err(error) => {
					let valid_up_to = position + error.valid_up_to();
					out.push_str(
						std::str::from_utf8(&self.pending[position..valid_up_to]).expect("prefix was validated"),
					);
					match error.error_len() {
						Some(length) => {
							out.push('\u{FFFD}');
							position = valid_up_to + length;
						}
						None if stream => {
							position = valid_up_to;
							break;
						}
						None => {
							out.push('\u{FFFD}');
							position = self.pending.len();
						}
					}
				}
			}
		}
		self.pending.drain(..position);
		if !self.started && !out.is_empty() {
			self.started = true;
			if let Some(rest) = out.strip_prefix('\u{FEFF}') {
				return rest.to_string();
			}
		}
		out
	}
}

/// `iterateAnthropicEvents(response, signal)` over `iterateSseMessages(body, signal)`: the parsed Anthropic message
/// events of the response, one per `next` call.
struct AnthropicEvents {
	body: Body,
	decoder: StreamingTextDecoder,
	state: SseDecoderState,
	buffer: String,
	queued: VecDeque<ServerSentEvent>,
	body_done: bool,
	saw_message_start: bool,
	saw_message_end: bool,
}

impl AnthropicEvents {
	fn new(response: HttpResponse) -> Self {
		Self {
			body: response.body,
			decoder: StreamingTextDecoder::default(),
			state: SseDecoderState::default(),
			buffer: String::new(),
			queued: VecDeque::new(),
			body_done: false,
			saw_message_start: false,
			saw_message_end: false,
		}
	}

	/// The next Anthropic event, `None` at the end of the stream.
	async fn next(&mut self, signal: Option<&AbortSignal>) -> PiResult<Option<Value>> {
		loop {
			if let Some(sse) = self.queued.pop_front() {
				match self.parse(sse)? {
					Some(event) => return Ok(Some(event)),
					None => continue,
				}
			}
			if self.body_done {
				if self.saw_message_start && !self.saw_message_end {
					self.saw_message_start = false;
					return Err(PiError::new("Anthropic stream ended before message_stop"));
				}
				return Ok(None);
			}

			if signal.is_some_and(AbortSignal::aborted) {
				return Err(PiError::new("Request was aborted"));
			}

			match self.body.next().await {
				Some(chunk) => {
					let text = self.decoder.decode(&chunk?, true);
					self.buffer.push_str(&text);
					self.consume_lines();
				}
				None => {
					let text = self.decoder.decode(&[], false);
					self.buffer.push_str(&text);
					self.consume_lines();
					if !self.buffer.is_empty() {
						let line = std::mem::take(&mut self.buffer);
						if let Some(event) = decode_sse_line(&line, &mut self.state) {
							self.queued.push_back(event);
						}
					}
					if let Some(trailing_event) = flush_sse_event(&mut self.state) {
						self.queued.push_back(trailing_event);
					}
					self.body_done = true;
				}
			}
		}
	}

	fn consume_lines(&mut self) {
		while let Some((line, rest)) = consume_line(&self.buffer) {
			let line = line.to_string();
			self.buffer.drain(..rest);
			if let Some(event) = decode_sse_line(&line, &mut self.state) {
				self.queued.push_back(event);
			}
		}
	}

	/// One server-sent event as an Anthropic event: `None` for events that are not message events.
	fn parse(&mut self, sse: ServerSentEvent) -> PiResult<Option<Value>> {
		if sse.event.as_deref() == Some("error") {
			return Err(PiError::new(sse.data));
		}

		if !ANTHROPIC_MESSAGE_EVENTS.contains(&sse.event.as_deref().unwrap_or("")) {
			return Ok(None);
		}

		let event = parse_json_with_repair(&sse.data).and_then(|event| {
			let event_type = js_get(Some(&event), "type")?.and_then(Value::as_str);
			if event_type == Some("message_start") {
				self.saw_message_start = true;
			} else if event_type == Some("message_stop") {
				self.saw_message_end = true;
			}
			Ok(event)
		});
		event.map(Some).map_err(|error| {
			PiError::new(format!(
				"Could not parse Anthropic SSE event {}: {}; data={}; raw={}",
				sse.event.as_deref().unwrap_or("null"),
				error.message,
				sse.data,
				sse.raw.join("\\n")
			))
		})
	}
}

/// `value[key]` of a property read that JS does not guard: `None` is `undefined`, and reading from `null` or
/// `undefined` is the `TypeError` JS throws.
fn js_get<'a>(value: Option<&'a Value>, key: &str) -> PiResult<Option<&'a Value>> {
	match value {
		None => Err(PiError::type_error(format!(
			"Cannot read properties of undefined (reading '{key}')"
		))),
		Some(Value::Null) => Err(PiError::type_error(format!(
			"Cannot read properties of null (reading '{key}')"
		))),
		Some(Value::Object(object)) => Ok(object.get(key)),
		Some(_) => Ok(None),
	}
}

/// `String(value)` of a property read, as `+=` appends it to a string: `"undefined"` for a missing property.
fn appended_string(value: Option<&Value>) -> String {
	match value {
		None => "undefined".into(),
		Some(value) => template_string(value),
	}
}

/// `value || 0` of a token count.
fn tokens_or_zero(value: Option<&Value>) -> f64 {
	value
		.and_then(Value::as_f64)
		.filter(|count| *count != 0.0 && !count.is_nan())
		.unwrap_or(0.0)
}

/// Builds the `*_start` event of a new content block.
type BlockStartEvent = fn(usize, SharedAssistantMessage) -> AssistantMessageEvent;

/// Stream state per Anthropic block index, kept off the content blocks so it never reaches partials.
struct OpenBlock {
	content_index: usize,
	arguments: Option<ToolCallArgumentsStream>,
}

/// `stream(model, context, options)`.
pub fn stream(
	model: &Model,
	context: &TranscriptContext,
	options: Option<AnthropicOptions>,
) -> AssistantMessageEventStream {
	let stream = AssistantMessageEventStream::new();
	let normalized_context = resolve_transcript(
		context,
		Some(get_anthropic_compat(model).supports_mid_convo_system_messages),
	);
	let current_tools = get_current_tools(&normalized_context.messages);
	let output: SharedAssistantMessage = Rc::new(RefCell::new(AssistantMessage {
		content: Vec::new(),
		api: model.api.clone(),
		provider: model.provider.clone(),
		model: model.id.clone(),
		stop_reason: StopReason::Pending,
		timestamp: pi_host::now_ms(),
		..AssistantMessage::default()
	}));

	let model = model.clone();
	let options = options.unwrap_or_default();
	let events = stream.clone();
	pi_host::spawn_local(async move {
		let Err(error) = run_stream(&model, &normalized_context, &current_tools, &options, &output, &events).await
		else {
			return;
		};
		let reason = if options.signal.as_ref().is_some_and(AbortSignal::aborted) {
			StopReason::Aborted
		} else {
			StopReason::Error
		};
		{
			let mut message = output.borrow_mut();
			message.stop_reason = reason;
			message.error_message = Some(error.message.clone());
			if reason == StopReason::Error {
				append_assistant_message_diagnostic(&mut message, create_provider_error_diagnostic(&error));
			}
		}
		events.push(AssistantMessageEvent::Error {
			reason,
			error: output.clone(),
		});
		events.end(None);
	});

	stream
}

/// The body of `stream`'s `try` block.
async fn run_stream(
	model: &Model,
	context: &TranscriptContext,
	current_tools: &[Tool],
	options: &AnthropicOptions,
	output: &SharedAssistantMessage,
	stream: &AssistantMessageEventStream,
) -> PiResult<()> {
	let mut usage_model = model.clone();
	let mut input_transformations: Option<Vec<Value>> = None;
	let api_version = get_anthropic_api_version(model, options);
	let model_headers = model.headers.as_ref().map(provider_headers);
	let auth_headers = merge_headers(&[model_headers.as_ref(), options.headers.as_ref()]);

	let (client, is_oauth): (Rc<dyn AnthropicClientApi>, bool) = match &options.client {
		Some(client) => (client.clone(), false),
		None => {
			let api_key = options.api_key.as_deref();
			let federation = get_anthropic_federation(model, api_key, Some(&auth_headers), options.env.as_ref());
			if federation.is_none() {
				assert_request_auth(&model.provider, api_key, Some(&auth_headers))?;
			}

			let copilot_dynamic_headers = (model.provider == "github-copilot").then(|| {
				let has_images = has_copilot_vision_input(&context.messages);
				build_copilot_dynamic_headers(&context.messages, has_images)
			});

			let cache_retention = resolve_cache_retention(options.cache_retention, options.env.as_ref());
			let cache_session_id = if cache_retention == CacheRetention::None {
				None
			} else {
				options.session_id.as_deref()
			};

			create_client(
				model,
				api_key,
				options.headers.as_ref(),
				options.fetch.clone(),
				copilot_dynamic_headers,
				cache_session_id,
				federation,
				options.identity.as_ref(),
			)?
		}
	};
	let request_options = RequestOptions {
		headers: api_version
			.clone()
			.map(|api_version| ProviderHeaders::from([("anthropic-version".to_string(), api_version)])),
		signal: options.signal.clone(),
		timeout: options.timeout_ms,
		max_retries: Some(0.0),
	};
	let mut disable_preferred_strict_tools = false;
	let response = loop {
		if let Some(signal) = &options.signal {
			signal.throw_if_aborted()?;
		}
		let built = build_params(
			model,
			context,
			is_oauth,
			api_version.as_ref().map(Option::as_deref),
			options,
			disable_preferred_strict_tools,
		)?;
		if let Some(level) = &built.provider_thinking_level {
			output.borrow_mut().provider_thinking_level = Some(level.clone());
		}
		let mut params = built.params;
		if let Some(on_payload) = &options.on_payload
			&& let Some(next_params) = on_payload(params.clone(), model).await?
		{
			let mut next_params = match next_params {
				Value::Object(next_params) => next_params,
				_ => Map::new(),
			};
			next_params.insert("stream".into(), Value::Bool(true));
			params = Value::Object(next_params);
		}
		let result = retry_provider_request(
			|| client.beta_messages_create(&params, request_options.clone()),
			ProviderRetryOptions {
				max_retries: options.max_retries,
				max_retry_delay_ms: options.max_retry_delay_ms,
				signal: options.signal.clone(),
				no_retry_statuses: None,
			},
		)
		.await;
		let error = match result {
			Ok(response) => break response,
			Err(error) => error,
		};
		// Only downgrade a rejected request, never an already-started stream.
		if options.signal.as_ref().is_some_and(AbortSignal::aborted)
			|| disable_preferred_strict_tools
			|| !built.has_preferred_strict_tools
			|| !is_api_error(&error)
			|| error.status != Some(400)
			|| error_type(&error) != Some("invalid_request_error")
			|| !is_strict_tool_limit_error_message(&error.message)
		{
			return Err(error);
		}
		disable_preferred_strict_tools = true;
		append_assistant_message_diagnostic(
			&mut output.borrow_mut(),
			AssistantMessageDiagnostic {
				r#type: "anthropic_strict_tools_fallback".into(),
				timestamp: pi_host::now_ms(),
				error: Some(DiagnosticErrorInfo {
					name: None,
					message: error.message.clone(),
					stack: None,
					code: None,
				}),
				details: Some(Map::from_iter([(
					"reason".to_string(),
					Value::from("strict_tool_limits_exceeded"),
				)])),
			},
		);
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
	stream.push(AssistantMessageEvent::Start {
		partial: output.clone(),
	});

	let mut open_blocks: HashMap<String, OpenBlock> = HashMap::new();
	let compat = model.anthropic_messages_compat();
	let mut events = AnthropicEvents::new(response);

	while let Some(event) = events.next(options.signal.as_ref()).await? {
		if let Some(on_provider_stream_event) = &options.on_provider_stream_event {
			on_provider_stream_event(&event, model).await?;
		}
		let index_key = json_stringify(event.get("index").unwrap_or(&Value::Null));
		match event.get("type").and_then(Value::as_str) {
			Some("message_start") => {
				let message = js_get(Some(&event), "message")?;
				let response_id = js_get(message, "id")?.and_then(Value::as_str).map(str::to_string);
				let message = message.expect("js_get read a property of it");
				let mut output = output.borrow_mut();
				output.response_id = response_id;
				if let Some(Value::Array(transformations)) = message.get("input_transformations") {
					input_transformations = Some(transformations.clone());
				}
				let response_model = message.get("model").and_then(Value::as_str);
				if response_model != Some(model.id.as_str()) {
					output.response_model = response_model.map(str::to_string);
				}
				let fallback_cost = response_model
					.filter(|response_model| *response_model != model.id)
					.and_then(|response_model| {
						compat
							.allowed_fallback_models
							.as_ref()?
							.iter()
							.find(|fallback| {
								model.provider == fallback.provider.as_str() && fallback.model == response_model
							})
							.map(|fallback| (response_model, fallback.cost.clone()))
					});
				usage_model = match fallback_cost {
					Some((response_model, cost)) => {
						let mut fallback_model = model.clone();
						fallback_model.id = response_model.to_string();
						fallback_model.cost = cost;
						fallback_model
					}
					None => model.clone(),
				};
				// Capture initial token usage from message_start event
				// This ensures we have input token counts even if the stream is aborted early
				let usage = message.get("usage");
				output.usage.input = tokens_or_zero(js_get(usage, "input_tokens")?);
				let usage = usage.expect("js_get read a property of it");
				output.usage.output = tokens_or_zero(usage.get("output_tokens"));
				output.usage.cache_read = tokens_or_zero(usage.get("cache_read_input_tokens"));
				output.usage.cache_write = tokens_or_zero(usage.get("cache_creation_input_tokens"));
				output.usage.cache_write_1h =
					Some(tokens_or_zero(usage.get("cache_creation").and_then(|cache_creation| {
						cache_creation.get("ephemeral_1h_input_tokens")
					})));
				// Anthropic doesn't provide total_tokens, compute from components
				output.usage.total_tokens =
					output.usage.input + output.usage.output + output.usage.cache_read + output.usage.cache_write;
				calculate_cost(&usage_model, &mut output.usage);
			}
			Some("content_block_start") => {
				let content_block = js_get(Some(&event), "content_block")?;
				let block_type = js_get(content_block, "type")?.and_then(Value::as_str);
				let content_block = content_block.expect("js_get read a property of it");
				if block_type == Some("fallback") {
					if !output.borrow().content.is_empty() {
						return Err(PiError::new(
							"Anthropic performed an unsupported mid-output model fallback",
						));
					}
					continue;
				}
				let nullish_string = |key: &str| match content_block.get(key) {
					None | Some(Value::Null) => String::new(),
					Some(value) => template_string(value),
				};
				let (block, arguments, start): (AssistantContent, Option<ToolCallArgumentsStream>, BlockStartEvent) =
					match block_type {
						Some("text") => (
							AssistantContent::Text(TextContent {
								text: nullish_string("text"),
								..TextContent::default()
							}),
							None,
							|content_index, partial| AssistantMessageEvent::TextStart { content_index, partial },
						),
						Some("thinking") => (
							AssistantContent::Thinking(ThinkingContent {
								thinking: nullish_string("thinking"),
								thinking_signature: Some(nullish_string("signature")),
								..ThinkingContent::default()
							}),
							None,
							|content_index, partial| AssistantMessageEvent::ThinkingStart { content_index, partial },
						),
						Some("redacted_thinking") => (
							AssistantContent::Thinking(ThinkingContent {
								thinking: "[Reasoning redacted]".into(),
								thinking_signature: content_block
									.get("data")
									.filter(|data| !data.is_null())
									.map(template_string),
								redacted: Some(true),
								..ThinkingContent::default()
							}),
							None,
							|content_index, partial| AssistantMessageEvent::ThinkingStart { content_index, partial },
						),
						Some("tool_use") => {
							let name = content_block.get("name").map(template_string).unwrap_or_default();
							(
								AssistantContent::ToolCall(ToolCall {
									id: content_block.get("id").map(template_string).unwrap_or_default(),
									name: if is_oauth {
										from_claude_code_name(&name, current_tools)
									} else {
										name
									},
									arguments: content_block
										.get("input")
										.and_then(Value::as_object)
										.cloned()
										.unwrap_or_default(),
									..ToolCall::default()
								}),
								Some(ToolCallArgumentsStream::new()),
								|content_index, partial| AssistantMessageEvent::ToolcallStart {
									content_index,
									partial,
								},
							)
						}
						_ => continue,
					};
				let content_index = {
					let mut output = output.borrow_mut();
					output.content.push(block);
					output.content.len() - 1
				};
				open_blocks.insert(
					index_key,
					OpenBlock {
						content_index,
						arguments,
					},
				);
				stream.push(start(content_index, output.clone()));
			}
			Some("content_block_delta") => {
				let delta = js_get(Some(&event), "delta")?;
				let delta_type = js_get(delta, "type")?.and_then(Value::as_str);
				let delta = delta.expect("js_get read a property of it");
				let Some(open) = open_blocks.get_mut(&index_key) else {
					continue;
				};
				let content_index = open.content_index;
				let mut message = output.borrow_mut();
				let block = &mut message.content[content_index];
				match (delta_type, block) {
					(Some("text_delta"), AssistantContent::Text(block)) => {
						let text = appended_string(delta.get("text"));
						block.text.push_str(&text);
						drop(message);
						stream.push(AssistantMessageEvent::TextDelta {
							content_index,
							delta: text,
							partial: output.clone(),
						});
					}
					(Some("thinking_delta"), AssistantContent::Thinking(block)) => {
						let thinking = appended_string(delta.get("thinking"));
						block.thinking.push_str(&thinking);
						drop(message);
						stream.push(AssistantMessageEvent::ThinkingDelta {
							content_index,
							delta: thinking,
							partial: output.clone(),
						});
					}
					(Some("input_json_delta"), AssistantContent::ToolCall(block)) => {
						let Some(arguments) = &mut open.arguments else {
							continue;
						};
						let partial_json = appended_string(delta.get("partial_json"));
						block.arguments = (*arguments.append(&partial_json)).clone();
						drop(message);
						stream.push(AssistantMessageEvent::ToolcallDelta {
							content_index,
							delta: partial_json,
							partial: output.clone(),
						});
					}
					(Some("signature_delta"), AssistantContent::Thinking(block)) => {
						let mut signature = block.thinking_signature.take().unwrap_or_default();
						signature.push_str(&appended_string(delta.get("signature")));
						block.thinking_signature = Some(signature);
					}
					_ => {}
				}
			}
			Some("content_block_stop") => {
				let Some(open) = open_blocks.remove(&index_key) else {
					continue;
				};
				let content_index = open.content_index;
				let mut message = output.borrow_mut();
				let event = match &mut message.content[content_index] {
					AssistantContent::Text(block) => AssistantMessageEvent::TextEnd {
						content_index,
						content: block.text.clone(),
						partial: output.clone(),
					},
					AssistantContent::Thinking(block) => AssistantMessageEvent::ThinkingEnd {
						content_index,
						content: block.thinking.clone(),
						partial: output.clone(),
					},
					AssistantContent::ToolCall(block) => {
						let Some(arguments) = &open.arguments else {
							continue;
						};
						let finished = arguments.finish();
						block.arguments = finished.arguments;
						if finished.arguments_error.is_some() {
							block.arguments_error = finished.arguments_error;
						}
						AssistantMessageEvent::ToolcallEnd {
							content_index,
							tool_call: block.clone(),
							partial: output.clone(),
						}
					}
				};
				drop(message);
				stream.push(event);
			}
			Some("message_delta") => {
				if let Some(Value::Array(transformations)) = event.get("input_transformations") {
					input_transformations = Some(transformations.clone());
				}
				let delta = js_get(Some(&event), "delta")?;
				let stop_reason = js_get(delta, "stop_reason")?;
				let delta = delta.expect("js_get read a property of it");
				let mut output = output.borrow_mut();
				if let Some(stop_reason) = stop_reason.filter(|reason| truthy(reason)) {
					let stop_reason = template_string(stop_reason);
					output.raw_stop_reason = Some(stop_reason.clone());
					let (mapped, error_message) = map_stop_reason(&stop_reason, delta.get("stop_details"))?;
					output.stop_reason = mapped;
					if let Some(error_message) = error_message {
						output.error_message = Some(error_message);
					}
				}
				// Only update usage fields if present (not null).
				// Preserves input_tokens from message_start when proxies omit it in message_delta.
				let usage = event.get("usage").filter(|usage| truthy(usage));
				if let Some(usage) = usage {
					let count = |value: Option<&Value>| value.filter(|value| !value.is_null()).and_then(Value::as_f64);
					if let Some(input) = count(usage.get("input_tokens")) {
						output.usage.input = input;
					}
					if let Some(output_tokens) = count(usage.get("output_tokens")) {
						output.usage.output = output_tokens;
					}
					if let Some(cache_read) = count(usage.get("cache_read_input_tokens")) {
						output.usage.cache_read = cache_read;
					}
					if let Some(cache_write) = count(usage.get("cache_creation_input_tokens")) {
						output.usage.cache_write = cache_write;
					}
					// Vercel AI Gateway includes the TTL breakdown in deltas, though the SDK only types it on message_start.
					if let Some(cache_write_1h) = count(
						usage
							.get("cache_creation")
							.and_then(|cache_creation| cache_creation.get("ephemeral_1h_input_tokens")),
					) {
						output.usage.cache_write_1h = Some(cache_write_1h);
					}
					// Anthropic reports reasoning tokens as a subset of output tokens.
					if let Some(thinking_tokens) = count(
						usage
							.get("output_tokens_details")
							.and_then(|details| details.get("thinking_tokens")),
					) {
						output.usage.reasoning = Some(thinking_tokens);
					}
				}
				// Anthropic doesn't provide total_tokens, compute from components
				output.usage.total_tokens =
					output.usage.input + output.usage.output + output.usage.cache_read + output.usage.cache_write;
				calculate_cost(&usage_model, &mut output.usage);
				if is_open_router_model(&model.base)
					&& let Some(usage) = usage
				{
					// OpenRouter adds billing fields to streamed message_delta usage at runtime,
					// although its OpenAPI schema and the Anthropic SDK do not type those fields.
					apply_open_router_reported_cost(&mut output.usage, usage);
				}
			}
			_ => {}
		}
	}

	if options.signal.as_ref().is_some_and(AbortSignal::aborted) {
		return Err(PiError::new("Request was aborted"));
	}

	let stop_reason = output.borrow().stop_reason;
	if stop_reason == StopReason::Pending {
		return Err(PiError::new("Anthropic stream ended without a stop reason"));
	}
	if stop_reason == StopReason::Aborted || stop_reason == StopReason::Error {
		let error_message = output
			.borrow()
			.error_message
			.clone()
			.filter(|message| !message.is_empty());
		return Err(PiError::new(
			error_message.unwrap_or_else(|| "An unknown error occurred".into()),
		));
	}
	if let Some(transformations) = input_transformations.filter(|transformations| !transformations.is_empty()) {
		let transformations: Vec<Value> = transformations
			.iter()
			.map(|transformation| {
				let mut details = Map::new();
				for key in ["type", "path", "reason"] {
					if let Some(value) = transformation.get(key).filter(|value| !value.is_null()) {
						details.insert(key.to_string(), value.clone());
					}
				}
				Value::Object(details)
			})
			.collect();
		append_assistant_message_diagnostic(
			&mut output.borrow_mut(),
			AssistantMessageDiagnostic {
				r#type: "anthropic_input_transformations".into(),
				timestamp: pi_host::now_ms(),
				error: None,
				details: Some(Map::from_iter([(
					"transformations".to_string(),
					Value::Array(transformations),
				)])),
			},
		);
	}

	stream.push(AssistantMessageEvent::Done {
		reason: stop_reason,
		message: output.clone(),
	});
	stream.end(None);
	Ok(())
}

/// `error instanceof Anthropic.APIError`: an error the SDK client built from a response or a failed request.
fn is_api_error(error: &PiError) -> bool {
	error.data.as_ref().is_some_and(|data| data.get("headers").is_some())
}

/// `APIError.type`.
fn error_type(error: &PiError) -> Option<&str> {
	error.data.as_ref()?.get("type")?.as_str()
}

/// Map ThinkingLevel to Anthropic effort levels for adaptive thinking.
/// Note: effort "max" is available on all adaptive-thinking Claude models, while native
/// "xhigh" is only available on Opus 4.7/4.8, Sonnet 5, and Fable 5.
fn map_thinking_level_to_effort(model: &Model, level: ThinkingLevel) -> AnthropicEffort {
	if let Some(Some(mapped)) = model
		.thinking_level_map
		.as_ref()
		.and_then(|map| map.get(ModelThinkingLevel::from(level)))
	{
		return mapped.to_string();
	}

	match level {
		ThinkingLevel::Minimal | ThinkingLevel::Low => "low",
		ThinkingLevel::Medium => "medium",
		ThinkingLevel::High => "high",
		_ => "high",
	}
	.to_string()
}

/// `streamSimple(model, context, options)`. `Err` is the synchronous throw of a missing request auth.
pub fn stream_simple(
	model: &Model,
	context: &TranscriptContext,
	options: Option<SimpleStreamOptions>,
) -> PiResult<AssistantMessageEventStream> {
	let model_headers = model.headers.as_ref().map(provider_headers);
	let headers = options.as_ref().and_then(|options| options.headers.as_ref());
	let api_key = options.as_ref().and_then(|options| options.api_key.as_deref());
	let env = options.as_ref().and_then(|options| options.env.as_ref());
	let auth_headers = merge_headers(&[model_headers.as_ref(), headers]);
	if get_anthropic_federation(model, api_key, Some(&auth_headers), env).is_none() {
		assert_request_auth(&model.provider, api_key, Some(&auth_headers))?;
	}

	let base = AnthropicOptions {
		base: build_base_options(model, context, options.as_ref(), api_key),
		tool_choice: options.as_ref().and_then(|options| options.tool_choice).map(|choice| {
			Value::from(match choice {
				ToolChoice::Auto => "auto",
				ToolChoice::None => "none",
			})
		}),
		..AnthropicOptions::default()
	};
	let Some(reasoning) = options.as_ref().and_then(|options| options.reasoning) else {
		return Ok(stream(
			model,
			context,
			Some(AnthropicOptions {
				thinking_enabled: Some(false),
				..base
			}),
		));
	};

	// For models with adaptive thinking: use an effort level.
	// For older models: use budget-based thinking.
	if model.anthropic_messages_compat().force_adaptive_thinking == Some(true) {
		let effort = map_thinking_level_to_effort(model, reasoning);
		return Ok(stream(
			model,
			context,
			Some(AnthropicOptions {
				thinking_enabled: Some(true),
				effort: Some(effort),
				..base
			}),
		));
	}

	// Undefined means the caller did not request an output cap; let the helper use the model cap.
	// Do not coerce to 0 here, or the thinking budget would become the entire max_tokens value.
	let adjusted = adjust_max_tokens_for_thinking(
		base.max_tokens,
		model.max_tokens,
		reasoning,
		options.as_ref().and_then(|options| options.thinking_budgets.as_ref()),
	);

	let max_tokens = clamp_max_tokens_to_context(model, context, adjusted.max_tokens);

	let mut base = base;
	base.max_tokens = Some(max_tokens);
	Ok(stream(
		model,
		context,
		Some(AnthropicOptions {
			thinking_enabled: Some(true),
			thinking_budget_tokens: Some(math_min(adjusted.thinking_budget, math_max(0.0, max_tokens - 1024.0))),
			..base
		}),
	))
}

fn is_oauth_token(api_key: &str) -> bool {
	api_key.contains("sk-ant-oat")
}

/// `createClient(...)`: the client and whether it authenticates with an OAuth token.
#[allow(clippy::too_many_arguments)]
fn create_client(
	model: &Model,
	api_key: Option<&str>,
	options_headers: Option<&ProviderHeaders>,
	fetch: Option<FetchFunction>,
	dynamic_headers: Option<IndexMap<String, String>>,
	session_id: Option<&str>,
	federation: Option<AnthropicConfig>,
	identity: Option<&ClientIdentity>,
) -> PiResult<(Rc<dyn AnthropicClientApi>, bool)> {
	let model_headers = model.headers.as_ref().map(provider_headers);
	let target = ClientIdentityTarget {
		provider: &model.provider,
		base_url: &model.base_url,
	};
	let browser_headers = ProviderHeaders::from([
		("accept".to_string(), Some("application/json".to_string())),
		(
			"anthropic-dangerous-direct-browser-access".to_string(),
			Some("true".to_string()),
		),
	]);

	// Copilot: Bearer auth.
	if model.provider == "github-copilot" {
		let identity_headers = provider_headers(&get_identity_headers(target, identity)?);
		let dynamic_headers = dynamic_headers.as_ref().map(provider_headers);
		let client = AnthropicClient::new(ClientOptions {
			api_key: None,
			auth_token: api_key.map(str::to_string),
			base_url: Some(model.base_url.clone()),
			dangerously_allow_browser: Some(true),
			fetch,
			default_headers: Some(merge_headers(&[
				Some(&identity_headers),
				Some(&browser_headers),
				model_headers.as_ref(),
				dynamic_headers.as_ref(),
				options_headers,
			])),
			..ClientOptions::default()
		})?;

		return Ok((Rc::new(client), false));
	}

	// OAuth: Bearer auth, Claude Code identity headers instead of the client identity.
	if let Some(api_key) = api_key.filter(|api_key| is_oauth_token(api_key)) {
		let mut oauth_headers = browser_headers.clone();
		oauth_headers.insert("user-agent".into(), Some(format!("claude-cli/{CLAUDE_CODE_VERSION}")));
		oauth_headers.insert("x-app".into(), Some("cli".into()));
		let client = AnthropicClient::new(ClientOptions {
			api_key: None,
			auth_token: Some(api_key.to_string()),
			base_url: Some(model.base_url.clone()),
			dangerously_allow_browser: Some(true),
			fetch,
			default_headers: Some(merge_headers(&[
				Some(&oauth_headers),
				model_headers.as_ref(),
				options_headers,
			])),
			..ClientOptions::default()
		})?;

		return Ok((Rc::new(client), true));
	}

	// API key, header-owned auth, or workload identity federation.
	let compat = get_anthropic_compat(model);
	let mut session_affinity_headers = ProviderHeaders::new();
	if let Some(session_id) = session_id.filter(|session_id| !session_id.is_empty())
		&& compat.send_session_affinity_headers
	{
		let header = if compat.session_affinity_format == Some(AnthropicSessionAffinityFormat::Openrouter) {
			"x-session-id"
		} else {
			"x-session-affinity"
		};
		session_affinity_headers.insert(header.into(), Some(session_id.to_string()));
	}
	let identity_headers = provider_headers(&get_identity_headers(target, identity)?);
	let key_headers = (model.provider != "anthropic").then(|| {
		ProviderHeaders::from([
			("X-Api-Key".to_string(), api_key.map(str::to_string)),
			("Authorization".to_string(), None),
		])
	});
	let default_headers = merge_headers(&[
		Some(&identity_headers),
		Some(&browser_headers),
		key_headers.as_ref(),
		Some(&session_affinity_headers),
		model_headers.as_ref(),
		options_headers,
	]);
	if let Some(federation) = federation {
		let key = json_stringify(&json!([model.base_url, federation]));
		let client = FEDERATION_CLIENT.with_borrow_mut(|cached| -> PiResult<Rc<AnthropicClient>> {
			let reusable = cached.as_ref().is_some_and(|cached| {
				cached.key == key
					&& match (&cached.fetch, &fetch) {
						(None, None) => true,
						(Some(cached), Some(fetch)) => Rc::ptr_eq(cached, fetch),
						_ => false,
					}
			});
			if !reusable {
				let client = AnthropicClient::new(ClientOptions {
					api_key: None,
					auth_token: None,
					config: Some(federation),
					base_url: Some(model.base_url.clone()),
					dangerously_allow_browser: Some(true),
					fetch: fetch.clone(),
					..ClientOptions::default()
				})?;
				*cached = Some(FederationClient {
					key,
					fetch,
					client: Rc::new(client),
				});
			}
			Ok(cached
				.as_ref()
				.expect("the federation client was just cached")
				.client
				.clone())
		})?;
		return Ok((Rc::new(client.with_default_headers(default_headers)?), false));
	}

	let client = AnthropicClient::new(ClientOptions {
		api_key: api_key.map(str::to_string),
		auth_token: None,
		base_url: Some(model.base_url.clone()),
		dangerously_allow_browser: Some(true),
		fetch,
		default_headers: Some(default_headers),
		..ClientOptions::default()
	})?;

	Ok((Rc::new(client), false))
}

/// `getBetaFeatures(model, context, isOAuthToken, nativeToolChanges, options)`.
fn get_beta_features(
	model: &Model,
	context: &TranscriptContext,
	is_oauth_token: bool,
	native_tool_changes: bool,
	options: &AnthropicOptions,
) -> Vec<String> {
	let mut configured_features: Option<Option<String>> = None;
	let model_headers = model.headers.as_ref().map(provider_headers);
	for headers in [model_headers.as_ref(), options.headers.as_ref()].into_iter().flatten() {
		for (name, value) in headers {
			if name.to_lowercase() == "anthropic-beta" {
				configured_features = Some(value.clone());
			}
		}
	}
	match configured_features {
		Some(None) => return Vec::new(),
		Some(Some(configured_features)) => {
			let features: IndexSet<String> = configured_features
				.split(',')
				.map(|feature| js_trim(feature).to_string())
				.filter(|feature| !feature.is_empty())
				.collect();
			return features.into_iter().collect();
		}
		None => {}
	}

	let compat = model.anthropic_messages_compat();
	let mut features: IndexSet<String> = IndexSet::new();
	if is_oauth_token {
		features.insert("claude-code-20250219".into());
		features.insert("oauth-2025-04-20".into());
	}
	if should_use_fine_grained_tool_streaming_beta(model, context) {
		features.insert(FINE_GRAINED_TOOL_STREAMING_BETA.into());
	}
	if model.reasoning
		&& options.thinking_enabled == Some(true)
		&& options.interleaved_thinking.unwrap_or(true)
		&& compat.force_adaptive_thinking != Some(true)
	{
		features.insert(INTERLEAVED_THINKING_BETA.into());
	}
	if should_use_server_side_fallback_beta(&compat) {
		features.insert(SERVER_SIDE_FALLBACK_BETA.into());
	}
	if compat.supports_mid_convo_effort == Some(true) {
		features.insert(MID_CONVERSATION_OUTPUT_CONFIG_BETA.into());
		features.insert(THINKING_BINDING_CONTROLS_BETA.into());
	}
	if native_tool_changes {
		features.insert(INLINE_TOOLS_BETA.into());
	}
	features.into_iter().collect()
}

/// `buildParams(...)`'s result.
struct BuiltParams {
	params: Value,
	has_preferred_strict_tools: bool,
	/// Effort of the final effort marker, recorded on the reply so later requests can replay it.
	provider_thinking_level: Option<AnthropicEffort>,
}

/// `buildParams(model, context, isOAuthToken, apiVersion, options, disablePreferredStrictTools)`. `api_version` is
/// `None` for `undefined` and `Some(None)` for `null`.
fn build_params(
	model: &Model,
	context: &TranscriptContext,
	is_oauth_token: bool,
	api_version: Option<Option<&str>>,
	options: &AnthropicOptions,
	disable_preferred_strict_tools: bool,
) -> PiResult<BuiltParams> {
	let cache_control = get_cache_control(model, options.cache_retention, options.env.as_ref());
	let compat = get_anthropic_compat(model);
	let raw_compat = model.anthropic_messages_compat();
	let initial_system_message = get_initial_system_message(&context.messages);
	let initial_system_text = initial_system_message.map(get_system_message_text).unwrap_or_default();
	let normalize = |id: &str, _: &Model, _: &AssistantMessage| normalize_tool_call_id(id);
	let transformed_messages = transform_messages(&context.messages, model, Some(&normalize));
	let conversation_messages = if initial_system_message.is_some() {
		&transformed_messages[1..]
	} else {
		&transformed_messages[..]
	};
	// Native tool changes keep the request-level tool list fixed and define every later tool
	// by value in a `tool_addition` block, which also expresses same-name redefinitions.
	// Anthropic rejects a tool list where every tool is deferred, so there must be an initial
	// active tool to anchor the placeholder. Otherwise the current tool list is sent.
	let has_preferred_strict_tools = Cell::new(false);
	let convert_tool_definitions = |tools: &[Tool], tool_cache_control: Option<&Value>| -> PiResult<Vec<Value>> {
		let definitions = convert_tools(
			tools,
			is_oauth_token,
			compat.supports_eager_tool_input_streaming,
			compat.supports_strict_tools,
			tool_cache_control,
			disable_preferred_strict_tools,
		)?;
		if definitions.iter().zip(tools).any(|(definition, tool)| {
			definition.get("strict") == Some(&Value::Bool(true)) && prefers_json_schema_strict_sampling(tool)
		}) {
			has_preferred_strict_tools.set(true);
		}
		Ok(definitions)
	};
	let initial_tools: &[Tool] = initial_system_message
		.and_then(|message| message.tools_added.as_deref())
		.unwrap_or_default();
	let native_tool_changes = compat.supports_mid_convo_system_messages
		&& compat.supports_mid_convo_tool_changes
		&& !initial_tools.is_empty();
	let thinking_disabled = model.reasoning
		&& options.thinking_enabled == Some(false)
		&& !matches!(model.thinking_level_map.as_ref().map(|map| &map.off), Some(Some(None)));
	// Effort cannot change between messages while thinking is disabled, so "off" sends no effort markers.
	let effort_messages = raw_compat.supports_mid_convo_effort == Some(true) && !thinking_disabled;
	let convert_added_tools = |tools: &[Tool]| convert_tool_definitions(tools, None);
	let converted = convert_messages(
		conversation_messages,
		is_oauth_token,
		cache_control.as_ref(),
		compat.allow_empty_signature,
		effort_messages.then_some(&*model.provider),
		native_tool_changes.then_some(&convert_added_tools as &ToolDefinitionConverter<'_>),
	)?;
	let active_effort = options.effort.clone().unwrap_or_else(|| "high".into());
	let beta_features = get_beta_features(model, context, is_oauth_token, native_tool_changes, options);
	let mut params = Map::new();
	params.insert("model".into(), model.id.clone().into());
	params.insert(
		"messages".into(),
		Value::Array(if effort_messages {
			insert_thinking_level_messages(converted, &active_effort)
		} else {
			converted.messages
		}),
	);
	params.insert(
		"max_tokens".into(),
		number_value(options.max_tokens.unwrap_or(model.max_tokens)),
	);
	params.insert("stream".into(), Value::Bool(true));
	if !beta_features.is_empty() {
		params.insert("betas".into(), beta_features.into());
	}

	let text_block = |text: &str| {
		let mut block = Map::new();
		block.insert("type".into(), "text".into());
		block.insert("text".into(), text.into());
		if let Some(cache_control) = &cache_control {
			block.insert("cache_control".into(), cache_control.clone());
		}
		Value::Object(block)
	};
	// For OAuth tokens, we MUST include Claude Code identity
	if is_oauth_token {
		let mut system = vec![text_block("You are Claude Code, Anthropic's official CLI for Claude.")];
		if !initial_system_text.is_empty() {
			system.push(text_block(sanitize_surrogates(&initial_system_text)));
		}
		params.insert("system".into(), Value::Array(system));
	} else if !initial_system_text.is_empty() {
		// Add cache control to system prompt for non-OAuth tokens
		params.insert(
			"system".into(),
			Value::Array(vec![text_block(sanitize_surrogates(&initial_system_text))]),
		);
	}

	// Temperature is incompatible with extended thinking and unsupported on Claude Opus 4.7+.
	if api_version == Some(Some("2023-06-01"))
		&& let Some(temperature) = options.temperature
		&& options.thinking_enabled != Some(true)
		&& raw_compat.supports_mid_convo_effort != Some(true)
		&& compat.supports_temperature
	{
		params.insert("temperature".into(), number_value(temperature));
	}

	let tool_cache_control = cache_control
		.as_ref()
		.filter(|_| compat.supports_cache_control_on_tools);
	if native_tool_changes {
		// Initial tools stay active with the cache breakpoint on the last one, followed by the
		// placeholder. The list never changes afterwards: later tools are defined by value in
		// `tool_addition` blocks and withdrawn by `tool_removal`, so the cached prefix survives
		// every tool change.
		let mut tools = convert_tool_definitions(initial_tools, tool_cache_control)?;
		tools.push(deferred_tool_placeholder());
		params.insert("tools".into(), Value::Array(tools));
	} else {
		let tools = get_current_tools(&context.messages);
		if !tools.is_empty() {
			params.insert(
				"tools".into(),
				Value::Array(convert_tool_definitions(&tools, tool_cache_control)?),
			);
		}
	}

	let display = || Value::from(options.thinking_display.clone().unwrap_or_else(|| "summarized".into()));
	if thinking_disabled {
		// No effort is sent, so the model default applies. Anthropic rejects `block_binding`
		// with disabled thinking.
		params.insert("thinking".into(), json!({ "type": "disabled" }));
	} else if effort_messages {
		// Managed effort models use adaptive thinking so prefix mismatches can
		// be dropped instead of surfacing as persistent 400 responses.
		params.insert(
			"thinking".into(),
			json!({
				"type": "adaptive",
				"display": display(),
				"block_binding": { "prefix_mismatch_behavior": "drop_block" },
			}),
		);
		// Effort markers set the level for every reply. The top-level effort is part of the cached
		// prompt, so it stays fixed. It is "low" because organizations can cap effort and reject
		// requests containing any level above the cap.
		params.insert("output_config".into(), json!({ "effort": "low" }));
	} else if model.reasoning && options.thinking_enabled == Some(true) {
		// Default to "summarized" so Opus 4.7 and Mythos Preview behave like
		// older Claude 4 models (whose API default is also "summarized").
		if raw_compat.force_adaptive_thinking == Some(true) {
			// Adaptive thinking: Claude decides when and how much to think.
			params.insert("thinking".into(), json!({ "type": "adaptive", "display": display() }));
			if let Some(effort) = options.effort.as_ref().filter(|effort| !effort.is_empty()) {
				params.insert("output_config".into(), json!({ "effort": effort }));
			}
		} else {
			// Budget-based thinking for older models
			let budget_tokens = options
				.thinking_budget_tokens
				.filter(|budget| *budget != 0.0 && !budget.is_nan())
				.unwrap_or(1024.0);
			params.insert(
				"thinking".into(),
				json!({
					"type": "enabled",
					"budget_tokens": number_value(budget_tokens),
					"display": display(),
				}),
			);
		}
	}

	if let Some(metadata) = &options.metadata
		&& let Some(Value::String(user_id)) = metadata.get("user_id")
	{
		params.insert("metadata".into(), json!({ "user_id": user_id }));
	}

	if let Some(tool_choice) = options.tool_choice.as_ref().filter(|choice| truthy(choice)) {
		let tool_choice = match tool_choice {
			Value::String(choice) => json!({ "type": choice }),
			choice => choice.clone(),
		};
		params.insert("tool_choice".into(), tool_choice);
	}

	if let Some(allowed_fallback_models) = raw_compat
		.allowed_fallback_models
		.as_ref()
		.filter(|models| !models.is_empty())
	{
		params.insert(
			"fallbacks".into(),
			allowed_fallback_models
				.iter()
				.map(|fallback| json!({ "model": fallback.model }))
				.collect(),
		);
	}

	Ok(BuiltParams {
		params: Value::Object(params),
		has_preferred_strict_tools: has_preferred_strict_tools.get(),
		provider_thinking_level: effort_messages.then_some(active_effort),
	})
}

// Normalize tool call IDs to match Anthropic's required pattern and length
fn normalize_tool_call_id(id: &str) -> String {
	// The TS regex has no `u` flag, so it replaces each UTF-16 code unit of a non-matching character.
	let mut normalized = String::new();
	for ch in id.chars() {
		if ch.is_ascii_alphanumeric() || ch == '_' || ch == '-' {
			normalized.push(ch);
		} else {
			normalized.extend(std::iter::repeat_n('_', ch.len_utf16()));
		}
	}
	normalized.truncate(64);
	normalized
}

fn convert_tool_result(msg: &ToolResultMessage, content: &[UserContent]) -> Value {
	json!({
		"type": "tool_result",
		"tool_use_id": msg.tool_call_id,
		"content": convert_content_blocks(content),
		"is_error": msg.is_error,
	})
}

/// Converts tool definitions for native `tool_addition` blocks.
type ToolDefinitionConverter<'a> = dyn Fn(&[Tool]) -> PiResult<Vec<Value>> + 'a;

/// `ConvertedAnthropicMessages`.
struct ConvertedAnthropicMessages {
	messages: Vec<Value>,
	assistant_levels: HashMap<usize, AnthropicEffort>,
}

/// `convertMessages(...)`. `convert_tool_definitions` converts tool definitions for native `tool_addition` blocks;
/// `None` when tool changes are not native.
fn convert_messages(
	transformed_messages: &[Message],
	is_oauth_token: bool,
	cache_control: Option<&Value>,
	allow_empty_signature: bool,
	managed_provider: Option<&str>,
	convert_tool_definitions: Option<&ToolDefinitionConverter<'_>>,
) -> PiResult<ConvertedAnthropicMessages> {
	let mut params: Vec<Value> = Vec::new();
	let mut assistant_levels: HashMap<usize, AnthropicEffort> = HashMap::new();
	// Later system messages are held back and emitted directly before the next assistant
	// message that is sent (or at the end of the transcript). Assistant messages that convert to
	// no blocks are dropped, so holding past them keeps an update from landing before a user
	// message (#11091). Anthropic requires `tool_result` blocks to
	// immediately follow their `tool_use`, so a system message between them is rejected; this
	// also mirrors where the managed-effort system messages are inserted. As a result an
	// update placed before a user message in the transcript lands after it on the wire.
	let mut pending_system_messages: Vec<Value> = Vec::new();

	let mut i = 0;
	while i < transformed_messages.len() {
		let msg = &transformed_messages[i];
		i += 1;

		match msg {
			Message::System(msg) => {
				// Later system messages only reach this point when the model accepts them natively;
				// otherwise the transcript was collapsed into the leading message before conversion.
				let text = render_system_message_update(msg);
				let mut blocks: Vec<Value> = Vec::new();
				if !text.is_empty() {
					blocks.push(json!({ "type": "text", "text": sanitize_surrogates(&text) }));
				}
				if let Some(convert_tool_definitions) = convert_tool_definitions {
					let added: &[Tool] = msg.tools_added.as_deref().unwrap_or_default();
					let redefined: IndexSet<&str> = added.iter().map(|tool| tool.name.as_str()).collect();
					for tool in msg.tools_removed.iter().flatten() {
						// A new definition under the same name replaces the old one, so no removal is needed.
						if redefined.contains(tool.name.as_str()) {
							continue;
						}
						let name = if is_oauth_token {
							to_claude_code_name(&tool.name)
						} else {
							tool.name.clone()
						};
						blocks.push(json!({
							"type": "tool_removal",
							"tool": { "type": "tool_reference", "name": name },
						}));
					}
					for definition in convert_tool_definitions(added)? {
						blocks.push(json!({
							"type": "tool_addition",
							"tool": { "type": "tool_definition", "definition": definition },
						}));
					}
				}
				if !blocks.is_empty() {
					pending_system_messages.push(json!({ "role": "system", "content": blocks }));
				}
			}
			Message::User(msg) => match &msg.content {
				UserMessageContent::Text(content) => {
					if !js_trim(content).is_empty() {
						params.push(json!({ "role": "user", "content": sanitize_surrogates(content) }));
					}
				}
				UserMessageContent::Blocks(content) => {
					let filtered_blocks: Vec<Value> = content
						.iter()
						.filter_map(|item| match item {
							UserContent::Text(item) => (!js_trim(&item.text).is_empty())
								.then(|| json!({ "type": "text", "text": sanitize_surrogates(&item.text) })),
							UserContent::Image(item) => Some(convert_image_block(item)),
						})
						.collect();
					if filtered_blocks.is_empty() {
						continue;
					}
					params.push(json!({ "role": "user", "content": filtered_blocks }));
				}
			},
			Message::Assistant(msg) => {
				let mut blocks: Vec<Value> = Vec::new();

				for block in &msg.content {
					match block {
						AssistantContent::Text(block) => {
							if js_trim(&block.text).is_empty() {
								continue;
							}
							blocks.push(json!({ "type": "text", "text": sanitize_surrogates(&block.text) }));
						}
						AssistantContent::Thinking(block) => {
							// Redacted thinking: pass the opaque payload back as redacted_thinking
							if block.redacted == Some(true) {
								let mut redacted = Map::new();
								redacted.insert("type".into(), "redacted_thinking".into());
								if let Some(data) = &block.thinking_signature {
									redacted.insert("data".into(), data.clone().into());
								}
								blocks.push(Value::Object(redacted));
								continue;
							}
							let thinking_signature = block
								.thinking_signature
								.as_deref()
								.filter(|signature| !js_trim(signature).is_empty());
							if js_trim(&block.thinking).is_empty() && thinking_signature.is_none() {
								continue;
							}
							// If thinking signature is missing/empty (e.g., from aborted stream),
							// convert to plain text for Anthropic. Some compatible providers emit
							// and accept empty signatures, so let marked models preserve the block.
							let thinking = sanitize_surrogates(&block.thinking);
							blocks.push(match thinking_signature {
								None if allow_empty_signature => {
									json!({ "type": "thinking", "thinking": thinking, "signature": "" })
								}
								None => json!({ "type": "text", "text": thinking }),
								Some(signature) => {
									json!({ "type": "thinking", "thinking": thinking, "signature": signature })
								}
							});
						}
						AssistantContent::ToolCall(block) => {
							let name = if is_oauth_token {
								to_claude_code_name(&block.name)
							} else {
								block.name.clone()
							};
							blocks.push(json!({
								"type": "tool_use",
								"id": block.id,
								"name": name,
								"input": block.arguments,
							}));
						}
					}
				}
				if blocks.is_empty() {
					continue;
				}
				params.append(&mut pending_system_messages);
				let message_index = params.len();
				params.push(json!({ "role": "assistant", "content": blocks }));
				if let Some(managed_provider) = managed_provider
					&& msg.api == "anthropic-messages"
					&& msg.provider == managed_provider
					&& let Some(level) = msg
						.provider_thinking_level
						.as_deref()
						.filter(|level| is_anthropic_effort(level))
				{
					assistant_levels.insert(message_index, level.to_string());
				}
			}
			Message::ToolResult(first) => {
				// Collect all consecutive toolResult messages, needed for z.ai Anthropic endpoint.
				let mut tool_results: Vec<Value> = Vec::new();
				// Anthropic rejects error tool results that contain anything but text, so their images follow
				// all tool results of the turn, which must come first in the user message (#10794).
				let mut error_images: Vec<Value> = Vec::new();
				let mut next = Some(first);
				while let Some(tool_msg) = next {
					match split_error_tool_result_images(tool_msg) {
						Some(split) => {
							let content: Vec<UserContent> = split.content.into_iter().map(UserContent::Text).collect();
							tool_results.push(convert_tool_result(tool_msg, &content));
							error_images.push(json!({ "type": "text", "text": split.caption }));
							error_images.extend(split.images.iter().map(convert_image_block));
						}
						None => tool_results.push(convert_tool_result(tool_msg, &tool_msg.content)),
					}
					next = match transformed_messages.get(i) {
						Some(Message::ToolResult(tool_msg)) => {
							i += 1;
							Some(tool_msg)
						}
						_ => None,
					};
				}

				tool_results.append(&mut error_images);
				params.push(json!({ "role": "user", "content": tool_results }));
			}
		}
	}

	params.append(&mut pending_system_messages);

	// Add cache_control to the last user or system message to cache conversation history
	if let Some(cache_control) = cache_control
		&& let Some(Value::Object(last_message)) = params.last_mut()
		&& matches!(
			last_message.get("role").and_then(Value::as_str),
			Some("user" | "system")
		) {
		match last_message.get_mut("content") {
			Some(Value::Array(content)) => {
				if let Some(Value::Object(last_block)) = content.last_mut()
					&& matches!(
						last_block.get("type").and_then(Value::as_str),
						Some("text" | "image" | "tool_result" | "tool_addition" | "tool_removal")
					) {
					last_block.insert("cache_control".into(), cache_control.clone());
				}
			}
			Some(Value::String(text)) => {
				let text = std::mem::take(text);
				last_message.insert(
					"content".into(),
					json!([{ "type": "text", "text": text, "cache_control": cache_control }]),
				);
			}
			_ => {}
		}
	}

	Ok(ConvertedAnthropicMessages {
		messages: params,
		assistant_levels,
	})
}

fn is_anthropic_effort(value: &str) -> bool {
	matches!(value, "low" | "medium" | "high" | "xhigh" | "max")
}

fn insert_thinking_level_messages(converted: ConvertedAnthropicMessages, active_effort: &str) -> Vec<Value> {
	let mut messages: Vec<Value> = Vec::new();
	for (index, message) in converted.messages.into_iter().enumerate() {
		if let Some(historical_effort) = converted.assistant_levels.get(&index) {
			messages.push(json!({ "role": "system", "content": [], "output_config": { "effort": historical_effort } }));
		}
		messages.push(message);
	}
	messages.push(json!({ "role": "system", "content": [], "output_config": { "effort": active_effort } }));
	messages
}

fn should_use_fine_grained_tool_streaming_beta(model: &Model, context: &TranscriptContext) -> bool {
	!get_current_tools(&context.messages).is_empty() && !get_anthropic_compat(model).supports_eager_tool_input_streaming
}

// Keywords Anthropic strict tool use rejects with a 400 for the whole request.
// https://platform.claude.com/docs/en/build-with-claude/structured-outputs#json-schema-limitations
const ANTHROPIC_STRICT_UNSUPPORTED_KEYWORDS: [&str; 11] = [
	"minimum",
	"maximum",
	"exclusiveMinimum",
	"exclusiveMaximum",
	"multipleOf",
	"maxItems",
	"uniqueItems",
	"minContains",
	"maxContains",
	"minProperties",
	"maxProperties",
];
const ANTHROPIC_STRICT_STRING_FORMATS: [&str; 10] = [
	"date-time",
	"time",
	"date",
	"duration",
	"email",
	"hostname",
	"uri",
	"ipv4",
	"ipv6",
	"uuid",
];

fn is_anthropic_strict_unsupported_keyword(key: &str, value: &Value) -> bool {
	if ANTHROPIC_STRICT_UNSUPPORTED_KEYWORDS.contains(&key) {
		return true;
	}
	if key == "minItems" {
		return value.as_f64().is_none_or(|count| count != 0.0 && count != 1.0);
	}
	if key == "format" {
		return value
			.as_str()
			.is_none_or(|format| !ANTHROPIC_STRICT_STRING_FORMATS.contains(&format));
	}
	false
}

fn convert_tools(
	tools: &[Tool],
	is_oauth_token: bool,
	supports_eager_tool_input_streaming: bool,
	supports_strict_tools: bool,
	cache_control: Option<&Value>,
	disable_preferred_strict_tools: bool,
) -> PiResult<Vec<Value>> {
	let is_unsupported_keyword: &UnsupportedStrictSchemaKeywordCheck = &is_anthropic_strict_unsupported_keyword;
	tools
		.iter()
		.enumerate()
		.map(|(index, tool)| {
			let strict = if disable_preferred_strict_tools && prefers_json_schema_strict_sampling(tool) {
				None
			} else {
				resolve_json_schema_strict_sampling(tool, supports_strict_tools, Some(is_unsupported_keyword))?
			};
			let parameters = get_json_schema_tool_parameters(tool, strict)?;
			let nullish = |key: &str, default: Value| match parameters.get(key) {
				None | Some(Value::Null) => default,
				Some(value) => value.clone(),
			};
			let mut input_schema = match (&parameters, strict) {
				(Value::Object(parameters), Some(true)) => parameters.clone(),
				_ => Map::new(),
			};
			input_schema.insert("type".into(), "object".into());
			input_schema.insert("properties".into(), nullish("properties", json!({})));
			input_schema.insert("required".into(), nullish("required", json!([])));

			let mut definition = Map::new();
			definition.insert(
				"name".into(),
				if is_oauth_token {
					to_claude_code_name(&tool.name)
				} else {
					tool.name.clone()
				}
				.into(),
			);
			definition.insert("description".into(), tool.description.clone().into());
			if supports_eager_tool_input_streaming {
				definition.insert("eager_input_streaming".into(), Value::Bool(true));
			}
			if strict == Some(true) {
				definition.insert("strict".into(), Value::Bool(true));
			}
			definition.insert("input_schema".into(), Value::Object(input_schema));
			if let Some(cache_control) = cache_control
				&& index == tools.len() - 1
			{
				definition.insert("cache_control".into(), cache_control.clone());
			}
			Ok(Value::Object(definition))
		})
		.collect()
}

/// `mapStopReason(reason, stopDetails)`: the stop reason and an error message.
fn map_stop_reason(reason: &str, stop_details: Option<&Value>) -> PiResult<(StopReason, Option<String>)> {
	Ok(match reason {
		"end_turn" => (StopReason::Stop, None),
		"max_tokens" => (StopReason::Length, None),
		"tool_use" => (StopReason::ToolUse, None),
		"refusal" => (
			StopReason::Error,
			Some(
				stop_details
					.and_then(|details| details.get("explanation"))
					.filter(|explanation| truthy(explanation))
					.map_or_else(|| "The model refused to complete the request".into(), template_string),
			),
		),
		// Stop is good enough -> resubmit
		"pause_turn" => (StopReason::Stop, None),
		// We don't supply stop sequences, so this should never happen
		"stop_sequence" => (StopReason::Stop, None),
		// Content flagged by safety filters (not yet in SDK types)
		"sensitive" => (StopReason::Error, Some("Provider stopped with: sensitive".into())),
		// Handle unknown stop reasons gracefully (API may add new values)
		_ => return Err(PiError::new(format!("Unhandled stop reason: {reason}"))),
	})
}

/// The module's `ProviderStreams`: `stream` reads the Anthropic options from `ProviderStreamOptions.extra`.
pub struct AnthropicMessagesStreams;

impl ProviderStreams for AnthropicMessagesStreams {
	fn stream(
		&self,
		model: &Model,
		context: &TranscriptContext,
		options: Option<ProviderStreamOptions>,
	) -> PiResult<AssistantMessageEventStream> {
		Ok(stream(
			model,
			context,
			options.map(AnthropicOptions::from_provider_options),
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
