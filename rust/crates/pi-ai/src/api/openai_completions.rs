//! Port of `packages/ai/src/api/openai-completions.ts`.
//! PORT-STATUS: reviewed
//!
//! The `openai` SDK is `api/openai_client.rs`. Differences from the TS:
//! - `OpenAICompletionsOptions.reasoningEffort` is a `ThinkingLevel`: through `ProviderStreamOptions.extra` an unknown
//!   effort string reads as absent, where the TS would send it as-is.
//! - Values the TS reads from stream chunks with loose typing are read with the JSON types the OpenAI wire format
//!   uses: a tool call `name`/`id`, a chunk `id`/`model` or a delta `content` that is not a string is ignored, a
//!   non-numeric usage field reads as 0, and a truthy `finish_reason` that is not a string is stored in
//!   `rawStopReason` as `String(value)`. A `tool_calls` value that is truthy but not an array fails with the
//!   `not iterable` TypeError (the TS iterates a string's characters). A truthy tool call `arguments` that is not a
//!   string is stringified before it becomes the `toolcall_delta` `delta`, which is a string. A chunk that is a JSON
//!   array is skipped; the TS reads it like an object, which only resets an empty `responseId`.
//! - When `onPayload` turns `stream` off, the parsed body is iterated as `for await` would: an array yields its items
//!   and a string its code points. An empty body reads as `null`, so it fails with the TS message for `null` where
//!   the TS reports `undefined`.
//! - `model.compat` is read through the typed compat schema: a value outside a field's TS type (an unknown
//!   `thinkingFormat`, `vllmPriority: null`) reads as absent, where the TS would use it as-is.
//! - Rust strings cannot hold a lone surrogate, so truncating an `openai` tool call id to 40 UTF-16 units stops
//!   before a surrogate pair the TS would split.
//! - The `prompt_cache_key` and `prompt_cache_retention` keys the TS declares as `undefined` are absent from the
//!   params `onPayload` sees; they keep their TS position when sampling params set them.
//! - Content blocks are tracked by their index in `output.content` instead of object identity; blocks are only ever
//!   appended, so the index is the TS `blocks.indexOf(block)`.

use std::cell::RefCell;
use std::collections::HashMap;
use std::ops::{Deref, DerefMut};
use std::rc::Rc;

use futures::StreamExt;
use indexmap::IndexMap;
use pi_host::js::{json_stringify, number_to_string, utf16_len, utf16_prefix};
use pi_host::{LocalBoxFuture, PiError, PiResult};
use serde_json::{Map, Value, json};

use crate::api::constrained_sampling::{
	GrammarToolInputJsonBuffer, append_grammar_tool_input_json_delta, close_grammar_tool_input_json,
	create_grammar_tool_input_properties, get_grammar_tool_input, get_json_schema_tool_parameters,
	resolve_grammar_constrained_sampling, resolve_json_schema_strict_sampling,
};
use crate::api::github_copilot_headers::{build_copilot_dynamic_headers, has_copilot_vision_input};
use crate::api::openai_client::{ApiData, ClientOptions, OpenAI, RequestOptions, is_truthy};
use crate::api::openai_prompt_cache::clamp_openai_prompt_cache_key;
use crate::api::simple_options::{
	build_base_options, clamp_thinking_budget_to_answer_room, resolve_sampling_params, thinking_budget_for_level,
};
use crate::api::transform_messages::transform_messages;
use crate::models::{calculate_cost, clamp_thinking_level};
use crate::providers::compat_schema::{CacheControlFormat, ChatTemplateVarName, MaxTokensField, ThinkingFormat};
use crate::types::{
	AssistantContent, AssistantMessage, AssistantMessageEvent, CacheRetention, ChatTemplateKwargValue, JsonObject,
	Message, Model, ModelInputModality, ModelThinkingLevel, ProviderEnv, ProviderHeaders, ProviderResponse,
	ProviderStreamOptions, ProviderStreams, SessionAffinityFormat, SharedAssistantMessage, SimpleStreamOptions,
	StopReason, StreamOptions, TextContent, ThinkingBudgets, ThinkingContent, ThinkingLevel, ThinkingTokenBudgetField,
	Tool, ToolCall, ToolChoice, TranscriptContext, Usage, UserContent, UserMessageContent,
};
use crate::utils::diagnostics::{append_assistant_message_diagnostic, create_provider_error_diagnostic};
use crate::utils::error_body::{format_provider_error, normalize_provider_error};
use crate::utils::event_stream::AssistantMessageEventStream;
use crate::utils::hash::short_hash;
use crate::utils::headers::headers_to_record;
use crate::utils::identity::{ClientIdentity, ClientIdentityTarget, get_identity_headers};
use crate::utils::json_parse::json_parse;
use crate::utils::openrouter_cost::{apply_open_router_reported_cost, is_open_router_model};
use crate::utils::provider_env::get_provider_env_value;
use crate::utils::provider_retry::{ProviderRetryOptions, retry_provider_request};
use crate::utils::sanitize_unicode::sanitize_surrogates;
use crate::utils::streaming_json::ToolCallArgumentsStream;
use crate::utils::text::{get_system_message_text, js_trim, render_system_message_update};
use crate::utils::transcript::{
	TranscriptToolSupport, get_declared_tools, resolve_transcript, resolve_transcript_tools,
};

/// `hasHeader(headers, name)`: a header of that name (any case) with a non-blank value.
fn has_header(headers: Option<&ProviderHeaders>, name: &str) -> bool {
	let Some(headers) = headers else {
		return false;
	};
	let expected = name.to_lowercase();
	headers.iter().any(|(key, value)| {
		key.to_lowercase() == expected && value.as_deref().is_some_and(|value| !js_trim(value).is_empty())
	})
}

fn get_client_api_key(provider: &str, api_key: Option<&str>, headers: Option<&ProviderHeaders>) -> PiResult<String> {
	if let Some(api_key) = api_key.filter(|api_key| !api_key.is_empty()) {
		return Ok(api_key.to_string());
	}
	if has_header(headers, "authorization") || has_header(headers, "cf-aig-authorization") {
		return Ok("unused".into());
	}
	Err(PiError::new(format!("No API key for provider: {provider}")))
}

/// Check if conversation messages contain tool calls or tool results.
/// This is needed because Anthropic (via proxy) requires the tools param
/// to be present when messages include tool_calls or tool role messages.
fn has_tool_history(messages: &[Message]) -> bool {
	for msg in messages {
		match msg {
			Message::ToolResult(_) => return true,
			Message::Assistant(msg)
				if msg
					.content
					.iter()
					.any(|block| matches!(block, AssistantContent::ToolCall(_))) =>
			{
				return true;
			}
			_ => {}
		}
	}
	false
}

fn has_valid_common_reasoning_detail_fields(candidate: &Map<String, Value>) -> bool {
	matches!(candidate.get("id"), None | Some(Value::Null) | Some(Value::String(_)))
		&& matches!(candidate.get("format"), None | Some(Value::String(_)))
		&& matches!(candidate.get("index"), None | Some(Value::Number(_)))
}

fn is_openai_reasoning_detail(detail: &Value) -> bool {
	let Some(detail) = detail.as_object() else {
		return false;
	};
	if !has_valid_common_reasoning_detail_fields(detail) {
		return false;
	}
	match detail.get("type").and_then(Value::as_str) {
		Some("reasoning.summary") => matches!(detail.get("summary"), Some(Value::String(_))),
		Some("reasoning.encrypted") => matches!(detail.get("data"), Some(Value::String(_))),
		Some("reasoning.text") => {
			matches!(detail.get("text"), Some(Value::String(_)))
				&& matches!(
					detail.get("signature"),
					None | Some(Value::Null) | Some(Value::String(_))
				)
		}
		_ => false,
	}
}

/// Options of the openai-completions `stream`.
#[derive(Clone, Default)]
pub struct OpenAICompletionsOptions {
	pub base: StreamOptions,
	/// `ChatCompletionToolChoiceOption`: `"none" | "auto" | "required"` or a named tool choice object.
	pub tool_choice: Option<Value>,
	pub reasoning_effort: Option<ThinkingLevel>,
	/// Token budgets per thinking level. Used when `compat.thinkingTokenBudgetField` or `compat.supportsThinkingTokenBudget` is set, or by `{ "$var": "thinking.budget" }`.
	pub thinking_budgets: Option<ThinkingBudgets>,
}

impl Deref for OpenAICompletionsOptions {
	type Target = StreamOptions;

	fn deref(&self) -> &Self::Target {
		&self.base
	}
}

impl DerefMut for OpenAICompletionsOptions {
	fn deref_mut(&mut self) -> &mut Self::Target {
		&mut self.base
	}
}

impl From<ProviderStreamOptions> for OpenAICompletionsOptions {
	/// Reads `toolChoice`, `reasoningEffort` and `thinkingBudgets` from the extra keys.
	fn from(options: ProviderStreamOptions) -> Self {
		let extra = options.extra;
		Self {
			base: options.base,
			tool_choice: extra.get("toolChoice").filter(|value| !value.is_null()).cloned(),
			reasoning_effort: extra
				.get("reasoningEffort")
				.and_then(|value| serde_json::from_value(value.clone()).ok()),
			thinking_budgets: extra
				.get("thinkingBudgets")
				.and_then(|value| serde_json::from_value(value.clone()).ok()),
		}
	}
}

/// `ConvertCompletionsMessagesOptions`.
#[derive(Clone, Copy, Debug, Default)]
pub struct ConvertCompletionsMessagesOptions<'a> {
	pub grammar_tool_input_properties: Option<&'a IndexMap<String, String>>,
}

/// `ResolvedOpenAICompletionsCompat`: `OpenAICompletionsCompat` with every field resolved, except the ones that stay
/// optional in the TS type.
#[derive(Clone, Debug, PartialEq)]
pub struct ResolvedOpenAICompletionsCompat {
	pub supports_store: bool,
	pub supports_developer_role: bool,
	pub supports_reasoning_effort: bool,
	pub supports_usage_in_streaming: bool,
	pub supports_finish_reason: bool,
	pub max_tokens_field: MaxTokensField,
	pub requires_tool_result_name: bool,
	pub requires_assistant_after_tool_result: bool,
	pub requires_thinking_as_text: bool,
	pub requires_reasoning_content_on_assistant_messages: bool,
	pub thinking_format: ThinkingFormat,
	/// The raw `openRouterRouting` object.
	pub open_router_routing: Value,
	/// The raw `vercelGatewayRouting` object.
	pub vercel_gateway_routing: Value,
	pub chat_template_kwargs: IndexMap<String, ChatTemplateKwargValue>,
	pub chat_template_args: IndexMap<String, ChatTemplateKwargValue>,
	pub zai_tool_stream: bool,
	pub supports_thinking_token_budget: Option<bool>,
	pub thinking_token_budget_field: Option<ThinkingTokenBudgetField>,
	pub supports_strict_mode: bool,
	pub supports_openai_grammar_tools: bool,
	pub supports_mid_convo_system_messages: Option<bool>,
	pub supports_mid_convo_tool_additions: Option<bool>,
	pub cache_control_format: Option<CacheControlFormat>,
	pub send_session_affinity_headers: bool,
	pub session_affinity_format: SessionAffinityFormat,
	pub supports_long_cache_retention: bool,
	pub vllm_priority: Option<f64>,
}

fn parse_openai_reasoning_details(signature: Option<&str>) -> Option<Vec<Value>> {
	let signature = signature.filter(|signature| !signature.is_empty())?;
	match json_parse(signature) {
		Ok(Value::Array(parsed)) if !parsed.is_empty() && parsed.iter().all(is_openai_reasoning_detail) => Some(parsed),
		_ => None,
	}
}

fn parse_legacy_encrypted_reasoning_detail(signature: Option<&str>) -> Option<Value> {
	let signature = signature.filter(|signature| !signature.is_empty())?;
	let parsed = json_parse(signature).ok()?;
	let valid = is_openai_reasoning_detail(&parsed)
		&& parsed.get("type").and_then(Value::as_str) == Some("reasoning.encrypted")
		&& parsed
			.get("id")
			.and_then(Value::as_str)
			.is_some_and(|id| !id.is_empty())
		&& parsed
			.get("data")
			.and_then(Value::as_str)
			.is_some_and(|data| !data.is_empty());
	valid.then_some(parsed)
}

/// A streamed `OpenAIReasoningDetail` as a JS object: property order, with `None` for a property assigned `undefined`
/// (kept in place, dropped by `JSON.stringify`).
#[derive(Clone, Debug, Default)]
struct ReasoningDetail {
	fields: IndexMap<String, Option<Value>>,
}

impl ReasoningDetail {
	fn get(&self, key: &str) -> Option<&Value> {
		self.fields.get(key).and_then(Option::as_ref)
	}

	fn detail_type(&self) -> Option<&str> {
		self.get("type").and_then(Value::as_str)
	}

	fn to_value(&self) -> Value {
		Value::Object(
			self.fields
				.iter()
				.filter_map(|(key, value)| value.as_ref().map(|value| (key.clone(), value.clone())))
				.collect(),
		)
	}
}

/// `target.key ??= source.key`.
fn assign_if_nullish(target: &mut ReasoningDetail, source: &Map<String, Value>, key: &str) {
	if matches!(target.get(key), None | Some(Value::Null)) {
		target.fields.insert(key.into(), source.get(key).cloned());
	}
}

/// `target.key ||= source.key`.
fn assign_if_falsy(target: &mut ReasoningDetail, source: &Map<String, Value>, key: &str) {
	if !target.get(key).is_some_and(is_truthy) {
		target.fields.insert(key.into(), source.get(key).cloned());
	}
}

fn fill_missing_common_reasoning_detail_fields(target: &mut ReasoningDetail, source: &Map<String, Value>) {
	assign_if_nullish(target, source, "id");
	assign_if_falsy(target, source, "format");
	assign_if_nullish(target, source, "index");
}

/// `target.key += source.key` for two strings.
fn append_string_field(target: &mut ReasoningDetail, source: &Map<String, Value>, key: &str) {
	let mut text = target.get(key).and_then(Value::as_str).unwrap_or_default().to_string();
	text.push_str(source.get(key).and_then(Value::as_str).unwrap_or_default());
	target.fields.insert(key.into(), Some(Value::String(text)));
}

fn append_openai_reasoning_detail(details: &mut Vec<ReasoningDetail>, detail: &Map<String, Value>) {
	let detail_type = detail.get("type").and_then(Value::as_str);
	if let Some(last_detail) = details.last_mut() {
		if detail_type == Some("reasoning.text") && last_detail.detail_type() == Some("reasoning.text") {
			append_string_field(last_detail, detail, "text");
			assign_if_falsy(last_detail, detail, "signature");
			fill_missing_common_reasoning_detail_fields(last_detail, detail);
			return;
		}
		if detail_type == Some("reasoning.summary") && last_detail.detail_type() == Some("reasoning.summary") {
			append_string_field(last_detail, detail, "summary");
			fill_missing_common_reasoning_detail_fields(last_detail, detail);
			return;
		}
	}
	details.push(ReasoningDetail {
		fields: detail
			.iter()
			.map(|(key, value)| (key.clone(), Some(value.clone())))
			.collect(),
	});
}

const OPENAI_COMPLETIONS_REASONING_FIELDS: [&str; 3] = ["reasoning", "reasoning_content", "reasoning_text"];

fn is_openai_completions_reasoning_field(field: &str) -> bool {
	OPENAI_COMPLETIONS_REASONING_FIELDS.contains(&field)
}

fn resolve_cache_retention(cache_retention: Option<CacheRetention>, env: Option<&ProviderEnv>) -> CacheRetention {
	if let Some(cache_retention) = cache_retention {
		return cache_retention;
	}
	if get_provider_env_value("PI_CACHE_RETENTION", env).as_deref() == Some("long") {
		return CacheRetention::Long;
	}
	CacheRetention::Short
}

/// `String(value)` of a JSON value.
fn js_to_string(value: &Value) -> String {
	match value {
		Value::Null => "null".into(),
		Value::Bool(value) => value.to_string(),
		Value::Number(number) => number_to_string(number.as_f64().unwrap_or(f64::NAN)),
		Value::String(value) => value.clone(),
		Value::Array(items) => items
			.iter()
			.map(|item| {
				if item.is_null() {
					String::new()
				} else {
					js_to_string(item)
				}
			})
			.collect::<Vec<_>>()
			.join(","),
		Value::Object(_) => "[object Object]".into(),
	}
}

/// A JSON number from a JS number (`NaN` and infinities become `null`, as `JSON.stringify` writes them). An integral
/// value is an integer, as it reads after a JSON round trip.
fn number_value(value: f64) -> Value {
	if value.fract() == 0.0 && value.abs() <= 9_007_199_254_740_991.0 {
		return Value::from(value as i64);
	}
	serde_json::Number::from_f64(value).map_or(Value::Null, Value::Number)
}

/// `new URL(url).hostname`, or `None` when `URL.parse` fails.
fn url_hostname(url: &str) -> Option<String> {
	let url = url::Url::parse(url).ok()?;
	Some(url.host_str().unwrap_or_default().to_string())
}

/// `stream`: streams one chat completion.
pub fn stream(
	model: &Model,
	context: &TranscriptContext,
	options: Option<OpenAICompletionsOptions>,
) -> AssistantMessageEventStream {
	let stream = AssistantMessageEventStream::new();
	let normalized_context = resolve_transcript(context, get_compat(model).supports_mid_convo_system_messages);
	let output: SharedAssistantMessage = Rc::new(RefCell::new(AssistantMessage {
		content: Vec::new(),
		api: model.api.clone(),
		provider: model.provider.clone(),
		model: model.id.clone(),
		usage: Usage::default(),
		stop_reason: StopReason::Pending,
		timestamp: pi_host::now_ms(),
		..AssistantMessage::default()
	}));
	let options = options.unwrap_or_default();
	// The TS async IIFE runs synchronously up to its first `await` (the `onPayload` result), so the client, the params
	// and the `onPayload` call happen before `stream` returns, and a failure among them ends the stream synchronously.
	let request = match prepare_request(model, &normalized_context, &options) {
		Ok(request) => request,
		Err(error) => {
			fail_stream(&stream, &output, &options, &RefCell::new(None), &error);
			return stream;
		}
	};
	let model = model.clone();
	let events = stream.clone();
	pi_host::spawn_local(async move {
		// `reasoning_details` are replay metadata, not user-visible stream deltas.
		// Keep them in memory during streaming and serialize once when the block is finalized.
		let streamed_reasoning_details: RefCell<Option<Vec<ReasoningDetail>>> = RefCell::new(None);
		let result = run_stream(&events, &model, request, &options, &output, &streamed_reasoning_details).await;
		if let Err(error) = result {
			fail_stream(&events, &output, &options, &streamed_reasoning_details, &error);
		}
	});
	stream
}

/// The `catch` block of `stream`: records `error` on the output and ends the stream with an `error` event.
fn fail_stream(
	events: &AssistantMessageEventStream,
	output: &SharedAssistantMessage,
	options: &OpenAICompletionsOptions,
	streamed_reasoning_details: &RefCell<Option<Vec<ReasoningDetail>>>,
	error: &PiError,
) {
	{
		let mut message = output.borrow_mut();
		for block in &mut message.content {
			if let AssistantContent::Thinking(block) = block {
				apply_streamed_reasoning_details(block, streamed_reasoning_details);
			}
		}
		let aborted = options.signal.as_ref().is_some_and(|signal| signal.aborted());
		message.stop_reason = if aborted {
			StopReason::Aborted
		} else {
			StopReason::Error
		};
		let mut error_message = format_provider_error(&normalize_provider_error(error), None);
		// Some providers via OpenRouter give additional information in this field.
		// normalizeProviderError already stringifies the parsed body (error.error)
		// into errorMessage, so only append the raw metadata when it is not already
		// present to avoid double-printing it.
		let raw_metadata = error
			.data
			.as_ref()
			.and_then(|data| data.get("error"))
			.and_then(|error| error.get("metadata"))
			.and_then(|metadata| metadata.get("raw"));
		if let Some(raw_metadata) = raw_metadata.filter(|raw| is_truthy(raw)) {
			let raw_metadata = js_to_string(raw_metadata);
			if !error_message.contains(&raw_metadata) {
				error_message.push('\n');
				error_message.push_str(&raw_metadata);
			}
		}
		message.error_message = Some(error_message);
		if message.stop_reason == StopReason::Error {
			append_assistant_message_diagnostic(&mut message, create_provider_error_diagnostic(error));
		}
	}
	let reason = output.borrow().stop_reason;
	events.push(AssistantMessageEvent::Error {
		reason,
		error: output.clone(),
	});
	events.end(None);
}

fn apply_streamed_reasoning_details(
	block: &mut ThinkingContent,
	streamed_reasoning_details: &RefCell<Option<Vec<ReasoningDetail>>>,
) {
	if let Some(details) = streamed_reasoning_details.borrow().as_ref() {
		let details: Vec<Value> = details.iter().map(ReasoningDetail::to_value).collect();
		block.thinking_signature = Some(json_stringify(&details));
	}
}

/// The synchronous start of the `try` block of `stream`: the client, the params and the pending `onPayload` result.
struct PreparedRequest {
	client: OpenAI,
	params: Value,
	next_params: Option<LocalBoxFuture<'static, PiResult<Option<Value>>>>,
	compat: ResolvedOpenAICompletionsCompat,
	grammar_tool_input_properties: IndexMap<String, String>,
}

fn prepare_request(
	model: &Model,
	context: &TranscriptContext,
	options: &OpenAICompletionsOptions,
) -> PiResult<PreparedRequest> {
	let api_key = get_client_api_key(&model.provider, options.api_key.as_deref(), options.headers.as_ref())?;
	let compat = get_compat(model);
	let grammar_tool_input_properties = create_grammar_tool_input_properties(
		Some(&get_declared_tools(&context.messages)),
		compat.supports_openai_grammar_tools,
	)?;
	let cache_retention = resolve_cache_retention(options.cache_retention, options.env.as_ref());
	let cache_session_id = if cache_retention == CacheRetention::None {
		None
	} else {
		options.session_id.as_deref()
	};
	let client = create_client(
		model,
		context,
		&api_key,
		options.headers.as_ref(),
		options.identity.as_ref(),
		options.fetch.clone(),
		cache_session_id,
		&compat,
	)?;
	let params = build_params(
		model,
		context,
		Some(options),
		&compat,
		cache_retention,
		&grammar_tool_input_properties,
	)?;
	let next_params = options
		.on_payload
		.as_ref()
		.map(|on_payload| on_payload(params.clone(), model));
	Ok(PreparedRequest {
		client,
		params,
		next_params,
		compat,
		grammar_tool_input_properties,
	})
}

/// The rest of the `try` block of `stream`, from the awaited `onPayload` result on.
async fn run_stream(
	stream: &AssistantMessageEventStream,
	model: &Model,
	request: PreparedRequest,
	options: &OpenAICompletionsOptions,
	output: &SharedAssistantMessage,
	streamed_reasoning_details: &RefCell<Option<Vec<ReasoningDetail>>>,
) -> PiResult<()> {
	let PreparedRequest {
		client,
		mut params,
		next_params,
		compat,
		grammar_tool_input_properties,
	} = request;
	if let Some(next_params) = next_params
		&& let Some(next_params) = next_params.await?
	{
		params = next_params;
	}
	let request_options = RequestOptions {
		signal: options.signal.clone(),
		timeout: options.timeout_ms,
		max_retries: Some(0.0),
		headers: None,
	};
	let with_response = retry_provider_request(
		|| client.chat_completions_create(&params, &request_options),
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
				status: with_response.response.status,
				headers: headers_to_record(&with_response.response.headers),
			},
			model,
		)
		.await?;
	}
	stream.push(AssistantMessageEvent::Start {
		partial: output.clone(),
	});

	// `for await` over a parsed body that is not a stream: an array yields its items and a string its code points.
	let mut openai_stream = match with_response.data {
		ApiData::Stream(openai_stream) => openai_stream,
		ApiData::Json(Value::Array(items)) => futures::stream::iter(items.into_iter().map(Ok)).boxed_local(),
		ApiData::Json(Value::String(text)) => {
			let characters: Vec<PiResult<Value>> = text
				.chars()
				.map(|character| Ok(Value::String(character.into())))
				.collect();
			futures::stream::iter(characters).boxed_local()
		}
		ApiData::Json(Value::Null) => {
			return Err(PiError::type_error(
				"Cannot read properties of null (reading 'Symbol(Symbol.asyncIterator)')",
			));
		}
		ApiData::Json(_) => return Err(PiError::type_error("openaiStream is not async iterable")),
	};
	let mut state = CompletionsStream {
		stream,
		output,
		streamed_reasoning_details,
		grammar_tool_input_properties: &grammar_tool_input_properties,
		text_block: None,
		thinking_block: None,
		tool_call_blocks_by_index: HashMap::new(),
		tool_call_blocks_by_id: HashMap::new(),
		tool_call_states: HashMap::new(),
	};
	let mut has_finish_reason = false;

	while let Some(chunk) = openai_stream.next().await {
		let chunk = chunk?;
		if let Some(on_provider_stream_event) = &options.on_provider_stream_event {
			on_provider_stream_event(&chunk, model).await?;
		}
		let Some(chunk) = chunk.as_object() else {
			continue;
		};

		{
			let mut message = output.borrow_mut();
			// OpenAI documents ChatCompletionChunk.id as the unique chat completion identifier,
			// and each chunk in a streamed completion carries the same id.
			if !message.response_id.as_deref().is_some_and(|id| !id.is_empty()) {
				message.response_id = chunk.get("id").and_then(Value::as_str).map(str::to_string);
			}
			if let Some(chunk_model) = chunk.get("model").and_then(Value::as_str)
				&& !chunk_model.is_empty()
				&& chunk_model != model.id
				&& !message.response_model.as_deref().is_some_and(|id| !id.is_empty())
			{
				message.response_model = Some(chunk_model.to_string());
			}
			if let Some(usage) = chunk.get("usage").filter(|usage| is_truthy(usage)) {
				message.usage = parse_chunk_usage(usage, model);
			}
		}

		let choice = match chunk.get("choices") {
			Some(Value::Array(choices)) => choices.first(),
			_ => None,
		};
		let Some(choice) = choice.filter(|choice| is_truthy(choice)) else {
			continue;
		};

		// Fallback: some providers (e.g., Moonshot) return usage
		// in choice.usage instead of the standard chunk.usage
		if !chunk.get("usage").is_some_and(is_truthy)
			&& let Some(usage) = choice.get("usage").filter(|usage| is_truthy(usage))
		{
			output.borrow_mut().usage = parse_chunk_usage(usage, model);
		}

		if let Some(finish_reason) = choice.get("finish_reason").filter(|reason| is_truthy(reason)) {
			let finish_reason = js_to_string(finish_reason);
			let (stop_reason, error_message) = map_stop_reason(&finish_reason);
			let mut message = output.borrow_mut();
			message.raw_stop_reason = Some(finish_reason);
			message.stop_reason = stop_reason;
			if let Some(error_message) = error_message {
				message.error_message = Some(error_message);
			}
			has_finish_reason = true;
		}

		let Some(delta) = choice.get("delta").filter(|delta| is_truthy(delta)) else {
			continue;
		};
		// Handle reasoning before content: some proxies (e.g. LiteLLM) aggregate reasoning and
		// answer into one delta, and the thinking block must precede the text block.
		// Some endpoints return reasoning in reasoning_content (llama.cpp),
		// or reasoning (other openai compatible endpoints)
		// Use the first non-empty reasoning field to avoid duplication
		// (e.g., chutes.ai returns both reasoning_content and reasoning with same content)
		let reasoning_fields = ["reasoning_content", "reasoning", "reasoning_text"];
		let found_reasoning_field = reasoning_fields.into_iter().find_map(|field| match delta.get(field) {
			Some(Value::String(value)) if !value.is_empty() => Some((field, value)),
			_ => None,
		});
		if let Some((field, reasoning_delta)) = found_reasoning_field {
			let thinking_signature = if model.provider == "opencode-go" && field == "reasoning" {
				"reasoning_content"
			} else {
				field
			};
			let index = state.ensure_thinking_block(thinking_signature);
			if let AssistantContent::Thinking(block) = &mut output.borrow_mut().content[index] {
				block.thinking.push_str(reasoning_delta);
			}
			stream.push(AssistantMessageEvent::ThinkingDelta {
				content_index: index,
				delta: reasoning_delta.clone(),
				partial: output.clone(),
			});
		}

		if let Some(Value::Array(reasoning_details)) = delta.get("reasoning_details") {
			for detail in reasoning_details {
				if !is_openai_reasoning_detail(detail) {
					continue;
				}
				state.ensure_thinking_block("");
				// Keep provider replay data in the existing signature slot. OpenRouter streams
				// reasoning_details as deltas: consecutive text/summary deltas are merged into
				// logical entries, while encrypted entries remain opaque and discrete.
				let mut details = streamed_reasoning_details.borrow_mut();
				append_openai_reasoning_detail(
					details.get_or_insert_with(Vec::new),
					detail.as_object().expect("a reasoning detail is an object"),
				);
			}
		}

		if let Some(Value::String(content)) = delta.get("content")
			&& !content.is_empty()
		{
			let index = state.ensure_text_block();
			if let AssistantContent::Text(block) = &mut output.borrow_mut().content[index] {
				block.text.push_str(content);
			}
			stream.push(AssistantMessageEvent::TextDelta {
				content_index: index,
				delta: content.clone(),
				partial: output.clone(),
			});
		}

		if let Some(tool_calls) = delta.get("tool_calls").filter(|tool_calls| is_truthy(tool_calls)) {
			let Value::Array(tool_calls) = tool_calls else {
				return Err(PiError::type_error("choice.delta.tool_calls is not iterable"));
			};
			for tool_call in tool_calls {
				state.apply_tool_call_delta(tool_call)?;
			}
		}
	}

	let block_count = output.borrow().content.len();
	for index in 0..block_count {
		state.finish_block(index)?;
	}
	if options.signal.as_ref().is_some_and(|signal| signal.aborted()) {
		return Err(PiError::new("Request was aborted"));
	}

	let stop_reason = {
		let mut message = output.borrow_mut();
		if message.stop_reason == StopReason::Aborted {
			return Err(PiError::new("Request was aborted"));
		}
		if !has_finish_reason && !compat.supports_finish_reason {
			message.stop_reason = if message
				.content
				.iter()
				.any(|block| matches!(block, AssistantContent::ToolCall(_)))
			{
				StopReason::ToolUse
			} else {
				StopReason::Stop
			};
		}
		if message.stop_reason == StopReason::Error {
			return Err(PiError::new(
				message
					.error_message
					.clone()
					.filter(|message| !message.is_empty())
					.unwrap_or_else(|| "Provider returned an error stop reason".into()),
			));
		}
		if (compat.supports_finish_reason && !has_finish_reason) || message.stop_reason == StopReason::Pending {
			return Err(PiError::new("Stream ended without finish_reason"));
		}
		message.stop_reason
	};

	stream.push(AssistantMessageEvent::Done {
		reason: stop_reason,
		message: output.clone(),
	});
	stream.end(None);
	Ok(())
}

/// Stream state of a tool call, kept off the content block so it never reaches partials: JSON arguments, or the
/// input of a grammar (custom) tool.
enum ToolCallInput {
	Json(ToolCallArgumentsStream),
	Custom {
		property: String,
		buffer: GrammarToolInputJsonBuffer,
	},
}

struct ToolCallState {
	stream_index: Option<f64>,
	input: ToolCallInput,
}

/// The block bookkeeping of one streamed response.
struct CompletionsStream<'a> {
	stream: &'a AssistantMessageEventStream,
	output: &'a SharedAssistantMessage,
	streamed_reasoning_details: &'a RefCell<Option<Vec<ReasoningDetail>>>,
	grammar_tool_input_properties: &'a IndexMap<String, String>,
	text_block: Option<usize>,
	thinking_block: Option<usize>,
	tool_call_blocks_by_index: HashMap<u64, usize>,
	tool_call_blocks_by_id: HashMap<String, usize>,
	tool_call_states: HashMap<usize, ToolCallState>,
}

/// A `Map` key for a JS number (`SameValueZero`).
fn number_key(value: f64) -> u64 {
	if value == 0.0 {
		0.0f64.to_bits()
	} else {
		value.to_bits()
	}
}

fn custom_input(property: String) -> ToolCallInput {
	ToolCallInput::Custom {
		property,
		buffer: GrammarToolInputJsonBuffer {
			input: String::new(),
			started: false,
			closed: false,
		},
	}
}

/// `{ [property]: value }`.
fn single_property(property: &str, value: &str) -> JsonObject {
	JsonObject::from_iter([(property.to_string(), Value::String(value.to_string()))])
}

/// `value?.[key]` that is neither `null` nor `undefined`.
fn present<'v>(value: Option<&'v Value>, key: &str) -> Option<&'v Value> {
	value.and_then(|value| value.get(key)).filter(|value| !value.is_null())
}

impl CompletionsStream<'_> {
	fn tool_call_mut<R>(&self, index: usize, update: impl FnOnce(&mut ToolCall) -> R) -> R {
		match &mut self.output.borrow_mut().content[index] {
			AssistantContent::ToolCall(block) => update(block),
			_ => unreachable!("tool call states index tool call blocks"),
		}
	}

	fn finish_block(&mut self, content_index: usize) -> PiResult<()> {
		let block = self.output.borrow().content[content_index].clone();
		match block {
			AssistantContent::Text(block) => {
				self.stream.push(AssistantMessageEvent::TextEnd {
					content_index,
					content: block.text,
					partial: self.output.clone(),
				});
			}
			AssistantContent::Thinking(_) => {
				let content = {
					let mut message = self.output.borrow_mut();
					let AssistantContent::Thinking(block) = &mut message.content[content_index] else {
						unreachable!("the block is a thinking block");
					};
					apply_streamed_reasoning_details(block, self.streamed_reasoning_details);
					block.thinking.clone()
				};
				self.stream.push(AssistantMessageEvent::ThinkingEnd {
					content_index,
					content,
					partial: self.output.clone(),
				});
			}
			AssistantContent::ToolCall(_) => {
				let state = self
					.tool_call_states
					.remove(&content_index)
					.expect("every tool call block has a stream state");
				match state.input {
					ToolCallInput::Custom { property, mut buffer } => {
						if let Some(delta) = close_grammar_tool_input_json(&mut buffer, &property, None)? {
							self.stream.push(AssistantMessageEvent::ToolcallDelta {
								content_index,
								delta,
								partial: self.output.clone(),
							});
						}
					}
					ToolCallInput::Json(arguments) => {
						let parsed = arguments.finish();
						self.tool_call_mut(content_index, |block| {
							block.arguments = parsed.arguments;
							if let Some(arguments_error) = parsed.arguments_error {
								block.arguments_error = Some(arguments_error);
							}
						});
					}
				}
				let tool_call = self.tool_call_mut(content_index, |block| block.clone());
				self.stream.push(AssistantMessageEvent::ToolcallEnd {
					content_index,
					tool_call,
					partial: self.output.clone(),
				});
			}
		}
		Ok(())
	}

	fn push_block(&self, block: AssistantContent) -> usize {
		let mut message = self.output.borrow_mut();
		message.content.push(block);
		message.content.len() - 1
	}

	fn ensure_text_block(&mut self) -> usize {
		if let Some(index) = self.text_block {
			return index;
		}
		let index = self.push_block(AssistantContent::Text(TextContent::default()));
		self.text_block = Some(index);
		self.stream.push(AssistantMessageEvent::TextStart {
			content_index: index,
			partial: self.output.clone(),
		});
		index
	}

	fn ensure_thinking_block(&mut self, thinking_signature: &str) -> usize {
		if let Some(index) = self.thinking_block {
			return index;
		}
		let index = self.push_block(AssistantContent::Thinking(ThinkingContent {
			thinking: String::new(),
			thinking_signature: Some(thinking_signature.to_string()),
			..ThinkingContent::default()
		}));
		self.thinking_block = Some(index);
		self.stream.push(AssistantMessageEvent::ThinkingStart {
			content_index: index,
			partial: self.output.clone(),
		});
		index
	}

	fn ensure_tool_call_block(&mut self, tool_call: &Value) -> usize {
		let stream_index = tool_call.get("index").and_then(Value::as_f64);
		let function = tool_call.get("function");
		let custom = tool_call.get("custom");
		let name = present(function, "name")
			.or_else(|| present(custom, "name"))
			.and_then(Value::as_str)
			.unwrap_or_default()
			.to_string();
		let id = tool_call.get("id").and_then(Value::as_str).filter(|id| !id.is_empty());
		let is_custom = custom.is_some_and(is_truthy) && !function.is_some_and(is_truthy);
		let mut index = stream_index
			.and_then(|stream_index| self.tool_call_blocks_by_index.get(&number_key(stream_index)).copied());
		if index.is_none()
			&& let Some(id) = id
		{
			index = self.tool_call_blocks_by_id.get(id).copied();
		}
		let index = match index {
			Some(index) => index,
			None => {
				// Note: the "input" fallback here should/must not be taken.  in case the LLM makes up
				// a tool we don't knwo about, we at least have a place to stash our stuff.
				let custom_input_property = is_custom.then(|| {
					self.grammar_tool_input_properties
						.get(&name)
						.cloned()
						.unwrap_or_else(|| "input".into())
				});
				let block = ToolCall {
					id: id.unwrap_or_default().to_string(),
					name: name.clone(),
					arguments: match &custom_input_property {
						Some(property) => single_property(property, ""),
						None => JsonObject::new(),
					},
					..ToolCall::default()
				};
				let index = self.push_block(AssistantContent::ToolCall(block));
				self.tool_call_states.insert(
					index,
					ToolCallState {
						stream_index,
						input: match custom_input_property {
							Some(property) => custom_input(property),
							None => ToolCallInput::Json(ToolCallArgumentsStream::new()),
						},
					},
				);
				if let Some(stream_index) = stream_index {
					self.tool_call_blocks_by_index.insert(number_key(stream_index), index);
				}
				if let Some(id) = id {
					self.tool_call_blocks_by_id.insert(id.to_string(), index);
				}
				self.stream.push(AssistantMessageEvent::ToolcallStart {
					content_index: index,
					partial: self.output.clone(),
				});
				index
			}
		};
		let state = self
			.tool_call_states
			.get_mut(&index)
			.expect("every tool call block has a stream state");
		if let Some(stream_index) = stream_index
			&& state.stream_index.is_none()
		{
			state.stream_index = Some(stream_index);
			self.tool_call_blocks_by_index.insert(number_key(stream_index), index);
		}
		if let Some(id) = id {
			self.tool_call_blocks_by_id.insert(id.to_string(), index);
		}
		let state = self
			.tool_call_states
			.get_mut(&index)
			.expect("every tool call block has a stream state");
		let properties = self.grammar_tool_input_properties;
		let mut message = self.output.borrow_mut();
		let AssistantContent::ToolCall(block) = &mut message.content[index] else {
			unreachable!("tool call states index tool call blocks");
		};
		if block.name.is_empty() && !name.is_empty() {
			block.name = name;
		}
		if is_custom && !matches!(state.input, ToolCallInput::Custom { .. }) {
			let custom_input_property = properties.get(&block.name).cloned().unwrap_or_else(|| "input".into());
			block.arguments = single_property(&custom_input_property, "");
			state.input = custom_input(custom_input_property);
		}
		index
	}

	fn apply_tool_call_delta(&mut self, tool_call: &Value) -> PiResult<()> {
		if tool_call.is_null() {
			return Err(PiError::type_error("Cannot read properties of null (reading 'index')"));
		}
		let index = self.ensure_tool_call_block(tool_call);
		let function = tool_call.get("function");
		let custom = tool_call.get("custom");
		if let Some(id) = tool_call.get("id").and_then(Value::as_str).filter(|id| !id.is_empty()) {
			let missing_id = self.tool_call_mut(index, |block| {
				let missing = block.id.is_empty();
				if missing {
					block.id = id.to_string();
				}
				missing
			});
			if missing_id {
				self.tool_call_blocks_by_id.insert(id.to_string(), index);
			}
		}
		let name = present(function, "name")
			.or_else(|| present(custom, "name"))
			.and_then(Value::as_str);
		if let Some(name) = name.filter(|name| !name.is_empty()) {
			self.tool_call_mut(index, |block| {
				if block.name.is_empty() {
					block.name = name.to_string();
				}
			});
		}

		let arguments = function
			.and_then(|function| function.get("arguments"))
			.filter(|value| is_truthy(value));
		let input = custom
			.and_then(|custom| custom.get("input"))
			.filter(|value| is_truthy(value));
		let state = self
			.tool_call_states
			.get_mut(&index)
			.expect("every tool call block has a stream state");
		let (delta, new_arguments) = match (&mut state.input, arguments, input) {
			(ToolCallInput::Json(stream), Some(arguments), _) => {
				let delta = js_to_string(arguments);
				match &mut self.output.borrow_mut().content[index] {
					AssistantContent::ToolCall(block) => stream.append(&delta, &mut block.arguments),
					_ => unreachable!("tool call states index tool call blocks"),
				}
				(delta, None)
			}
			(ToolCallInput::Custom { property, buffer }, _, Some(input)) => {
				let delta =
					append_grammar_tool_input_json_delta(buffer, property, &js_to_string(input))?.unwrap_or_default();
				(delta, Some(single_property(property, &buffer.input)))
			}
			_ => (String::new(), None),
		};
		if let Some(arguments) = new_arguments {
			self.tool_call_mut(index, |block| block.arguments = arguments);
		}
		self.stream.push(AssistantMessageEvent::ToolcallDelta {
			content_index: index,
			delta,
			partial: self.output.clone(),
		});
		Ok(())
	}
}

/// `streamSimple`: `stream` with the provider-neutral simple options.
pub fn stream_simple(
	model: &Model,
	context: &TranscriptContext,
	options: Option<SimpleStreamOptions>,
) -> PiResult<AssistantMessageEventStream> {
	get_client_api_key(
		&model.provider,
		options.as_ref().and_then(|options| options.api_key.as_deref()),
		options.as_ref().and_then(|options| options.headers.as_ref()),
	)?;

	let api_key = options.as_ref().and_then(|options| options.api_key.clone());
	let base = build_base_options(model, context, options.as_ref(), api_key.as_deref());
	// The TS forwards `options.toolChoice` unchecked, so a value outside `ToolChoice` (`"required"`) arrives as an
	// extra key.
	let tool_choice = options.as_ref().and_then(|options| {
		options
			.tool_choice
			.map(|tool_choice| Value::String(tool_choice_name(tool_choice).into()))
			.or_else(|| {
				options
					.extra
					.get("toolChoice")
					.filter(|value| !value.is_null())
					.cloned()
			})
	});
	let clamped_reasoning = options
		.as_ref()
		.and_then(|options| options.reasoning)
		.map(|reasoning| clamp_thinking_level(model, ModelThinkingLevel::from(reasoning)));
	let reasoning_effort = clamped_reasoning.and_then(thinking_level);

	Ok(stream(
		model,
		context,
		Some(OpenAICompletionsOptions {
			base,
			tool_choice,
			reasoning_effort,
			thinking_budgets: options.and_then(|options| options.thinking_budgets),
		}),
	))
}

fn tool_choice_name(tool_choice: ToolChoice) -> &'static str {
	match tool_choice {
		ToolChoice::Auto => "auto",
		ToolChoice::None => "none",
	}
}

/// A model thinking level other than `off`.
fn thinking_level(level: ModelThinkingLevel) -> Option<ThinkingLevel> {
	match level {
		ModelThinkingLevel::Off => None,
		ModelThinkingLevel::Minimal => Some(ThinkingLevel::Minimal),
		ModelThinkingLevel::Low => Some(ThinkingLevel::Low),
		ModelThinkingLevel::Medium => Some(ThinkingLevel::Medium),
		ModelThinkingLevel::High => Some(ThinkingLevel::High),
		ModelThinkingLevel::Xhigh => Some(ThinkingLevel::Xhigh),
		ModelThinkingLevel::Max => Some(ThinkingLevel::Max),
	}
}

/// The openai-completions API module as `ProviderStreams`.
pub struct OpenAICompletionsStreams;

impl ProviderStreams for OpenAICompletionsStreams {
	fn stream(
		&self,
		model: &Model,
		context: &TranscriptContext,
		options: Option<ProviderStreamOptions>,
	) -> PiResult<AssistantMessageEventStream> {
		Ok(stream(model, context, options.map(OpenAICompletionsOptions::from)))
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

#[allow(clippy::too_many_arguments)]
fn create_client(
	model: &Model,
	context: &TranscriptContext,
	api_key: &str,
	options_headers: Option<&ProviderHeaders>,
	identity: Option<&ClientIdentity>,
	fetch: Option<pi_host::FetchFunction>,
	session_id: Option<&str>,
	compat: &ResolvedOpenAICompletionsCompat,
) -> PiResult<OpenAI> {
	let mut headers: ProviderHeaders = IndexMap::new();
	let identity_headers = get_identity_headers(
		ClientIdentityTarget {
			provider: &model.provider,
			base_url: &model.base_url,
		},
		identity,
	)?;
	for (name, value) in identity_headers {
		headers.insert(name, Some(value));
	}
	for (name, value) in model.headers.iter().flatten() {
		headers.insert(name.clone(), Some(value.clone()));
	}
	if model.provider == "github-copilot" {
		let has_images = has_copilot_vision_input(&context.messages);
		for (name, value) in build_copilot_dynamic_headers(&context.messages, has_images) {
			headers.insert(name, Some(value));
		}
	}

	if let Some(session_id) = session_id.filter(|session_id| !session_id.is_empty())
		&& compat.send_session_affinity_headers
	{
		if compat.session_affinity_format == SessionAffinityFormat::Openrouter {
			headers.insert("x-session-id".into(), Some(session_id.into()));
		} else {
			if compat.session_affinity_format == SessionAffinityFormat::Openai {
				headers.insert("session_id".into(), Some(session_id.into()));
			}
			headers.insert("x-client-request-id".into(), Some(session_id.into()));
			headers.insert("x-session-affinity".into(), Some(session_id.into()));
		}
	}

	// Merge options headers last so they can override defaults
	if let Some(options_headers) = options_headers {
		for (name, value) in options_headers {
			headers.insert(name.clone(), value.clone());
		}
	}

	OpenAI::new(ClientOptions {
		api_key: Some(api_key.to_string()),
		base_url: Some(model.base_url.clone()),
		fetch,
		default_headers: Some(headers),
		..ClientOptions::default()
	})
}

/// `model.thinkingLevelMap?.[level]`.
fn mapped_level(model: &Model, level: ModelThinkingLevel) -> Option<Option<&str>> {
	model.thinking_level_map.as_ref().and_then(|map| map.get(level))
}

/// `model.thinkingLevelMap?.[effort] ?? effort`.
fn mapped_effort_or_effort(model: &Model, effort: ThinkingLevel) -> String {
	match mapped_level(model, effort.into()) {
		Some(Some(mapped)) => mapped.to_string(),
		_ => effort.as_str().to_string(),
	}
}

fn build_params(
	model: &Model,
	context: &TranscriptContext,
	options: Option<&OpenAICompletionsOptions>,
	compat: &ResolvedOpenAICompletionsCompat,
	cache_retention: CacheRetention,
	grammar_tool_input_properties: &IndexMap<String, String>,
) -> PiResult<Value> {
	let transcript_tools = resolve_transcript_tools(&context.messages, transcript_tool_support(compat));
	let mut messages = convert_messages(
		model,
		context,
		compat,
		Some(ConvertCompletionsMessagesOptions {
			grammar_tool_input_properties: Some(grammar_tool_input_properties),
		}),
	)?;
	let cache_control = get_compat_cache_control(compat, cache_retention);
	let effort = options.and_then(|options| options.reasoning_effort);

	let mut params = Map::new();
	params.insert("model".into(), Value::String(model.id.clone()));
	params.insert("messages".into(), Value::Null);
	params.insert("stream".into(), Value::Bool(true));
	let long_cache_retention = cache_retention == CacheRetention::Long && compat.supports_long_cache_retention;
	let prompt_cache_key = if (url_hostname(&model.base_url).as_deref() == Some("api.openai.com")
		&& cache_retention != CacheRetention::None)
		|| long_cache_retention
	{
		clamp_openai_prompt_cache_key(options.and_then(|options| options.session_id.as_deref()))
	} else {
		None
	};
	// The TS literal declares both keys even when they are `undefined`, so a later `Object.assign` of sampling params
	// fills them in place. Unset keys hold a `null` placeholder here and are removed at the end.
	let mut unset_keys: Vec<&str> = Vec::new();
	match prompt_cache_key {
		Some(prompt_cache_key) => {
			params.insert("prompt_cache_key".into(), Value::String(prompt_cache_key));
		}
		None => {
			params.insert("prompt_cache_key".into(), Value::Null);
			unset_keys.push("prompt_cache_key");
		}
	}
	if long_cache_retention {
		params.insert("prompt_cache_retention".into(), Value::String("24h".into()));
	} else {
		params.insert("prompt_cache_retention".into(), Value::Null);
		unset_keys.push("prompt_cache_retention");
	}

	if compat.supports_usage_in_streaming {
		params.insert("stream_options".into(), json!({ "include_usage": true }));
	}

	if compat.supports_store {
		params.insert("store".into(), Value::Bool(false));
	}

	if let Some(max_tokens) = options
		.and_then(|options| options.max_tokens)
		.filter(|max_tokens| *max_tokens != 0.0 && !max_tokens.is_nan())
	{
		if compat.max_tokens_field == MaxTokensField::MaxTokens {
			// Deprecated by OpenAI, but some OpenAI-compatible providers only accept max_tokens.
			params.insert("max_tokens".into(), number_value(max_tokens));
		} else {
			params.insert("max_completion_tokens".into(), number_value(max_tokens));
		}
	}

	if let Some(temperature) = options.and_then(|options| options.temperature) {
		params.insert("temperature".into(), number_value(temperature));
	}

	let mut tools: Option<Vec<Value>> = None;
	if !transcript_tools.request_tools.is_empty() {
		tools = Some(convert_tools(&transcript_tools.request_tools, compat)?);
		params.insert("tools".into(), Value::Null);
		if compat.zai_tool_stream {
			params.insert("tool_stream".into(), Value::Bool(true));
		}
	} else if has_tool_history(&context.messages) {
		// Anthropic (via LiteLLM/proxy) requires tools param when conversation has tool_calls/tool_results
		tools = Some(Vec::new());
		params.insert("tools".into(), Value::Null);
	}

	if let Some(cache_control) = &cache_control {
		apply_anthropic_cache_control(&mut messages, tools.as_mut(), cache_control);
	}
	params.insert("messages".into(), Value::Array(messages));
	if let Some(tools) = tools {
		params.insert("tools".into(), Value::Array(tools));
	}

	if let Some(tool_choice) = options
		.and_then(|options| options.tool_choice.as_ref())
		.filter(|tool_choice| is_truthy(tool_choice))
	{
		params.insert("tool_choice".into(), tool_choice.clone());
	}

	if let Some(vllm_priority) = compat.vllm_priority {
		params.insert("priority".into(), number_value(vllm_priority));
	}

	let thinking_token_budget_field = resolve_thinking_token_budget_field(compat);
	let thinking_budget = resolve_clamped_thinking_budget(model, options, &params);

	match compat.thinking_format {
		ThinkingFormat::Zai if model.reasoning => {
			params.insert(
				"thinking".into(),
				if effort.is_some() {
					json!({ "type": "enabled", "clear_thinking": false })
				} else {
					json!({ "type": "disabled" })
				},
			);
			if let Some(effort) = effort
				&& compat.supports_reasoning_effort
			{
				let effort = match mapped_level(model, effort.into()) {
					None => Some(effort.as_str()),
					Some(mapped) => mapped,
				};
				if let Some(effort) = effort {
					params.insert("reasoning_effort".into(), Value::String(effort.into()));
				}
			}
		}
		ThinkingFormat::Qwen if model.reasoning => {
			params.insert("enable_thinking".into(), Value::Bool(effort.is_some()));
			if let Some(effort) = effort
				&& compat.supports_reasoning_effort
			{
				params.insert(
					"reasoning_effort".into(),
					Value::String(mapped_effort_or_effort(model, effort)),
				);
			}
		}
		ThinkingFormat::QwenChatTemplate if model.reasoning => {
			params.insert(
				"chat_template_kwargs".into(),
				json!({ "enable_thinking": effort.is_some(), "preserve_thinking": true }),
			);
		}
		ThinkingFormat::ChatTemplate if model.reasoning => {
			if let Some(chat_template_kwargs) =
				build_chat_template_values(model, effort, &compat.chat_template_kwargs, thinking_budget)
			{
				params.insert("chat_template_kwargs".into(), Value::Object(chat_template_kwargs));
			}
		}
		ThinkingFormat::Baseten if model.reasoning => {
			if let Some(chat_template_args) =
				build_chat_template_values(model, effort, &compat.chat_template_args, thinking_budget)
			{
				params.insert("chat_template_args".into(), Value::Object(chat_template_args));
			}
			if compat.supports_reasoning_effort {
				let mapped_effort = match effort {
					Some(effort) => mapped_level(model, effort.into()),
					None => mapped_level(model, ModelThinkingLevel::Off),
				};
				let effort = match mapped_effort {
					None => effort.map(ThinkingLevel::as_str),
					Some(mapped) => mapped,
				};
				if let Some(effort) = effort {
					params.insert("reasoning_effort".into(), Value::String(effort.into()));
				}
			}
		}
		ThinkingFormat::Deepseek if model.reasoning => {
			if effort.is_some() {
				params.insert("thinking".into(), json!({ "type": "enabled" }));
			} else if mapped_level(model, ModelThinkingLevel::Off) != Some(None) {
				params.insert("thinking".into(), json!({ "type": "disabled" }));
			}
			if let Some(effort) = effort
				&& compat.supports_reasoning_effort
			{
				params.insert(
					"reasoning_effort".into(),
					Value::String(mapped_effort_or_effort(model, effort)),
				);
			}
		}
		ThinkingFormat::Openrouter if model.reasoning => {
			// OpenRouter normalizes reasoning across providers via a nested reasoning object.
			if let Some(effort) = effort {
				params.insert(
					"reasoning".into(),
					json!({ "effort": mapped_effort_or_effort(model, effort) }),
				);
			} else if let Some(off) = mapped_level(model, ModelThinkingLevel::Off).unwrap_or(Some("none")) {
				params.insert("reasoning".into(), json!({ "effort": off }));
			}
		}
		ThinkingFormat::AntLing if model.reasoning && effort.is_some() => {
			let effort = effort.expect("the guard checked the effort");
			if let Some(Some(effort)) = mapped_level(model, effort.into()) {
				params.insert("reasoning".into(), json!({ "effort": effort }));
			}
		}
		ThinkingFormat::Together if model.reasoning => {
			params.insert("reasoning".into(), json!({ "enabled": effort.is_some() }));
			if let Some(effort) = effort
				&& compat.supports_reasoning_effort
			{
				params.insert(
					"reasoning_effort".into(),
					Value::String(mapped_effort_or_effort(model, effort)),
				);
			}
		}
		ThinkingFormat::StringThinking if model.reasoning => {
			if let Some(effort) = effort {
				params.insert("thinking".into(), Value::String(mapped_effort_or_effort(model, effort)));
			} else if let Some(off) = mapped_level(model, ModelThinkingLevel::Off).unwrap_or(Some("none")) {
				params.insert("thinking".into(), Value::String(off.into()));
			}
		}
		_ if effort.is_some() && model.reasoning && compat.supports_reasoning_effort => {
			// OpenAI-style reasoning_effort
			let effort = effort.expect("the guard checked the effort");
			params.insert(
				"reasoning_effort".into(),
				Value::String(mapped_effort_or_effort(model, effort)),
			);
		}
		_ if effort.is_none() && model.reasoning && compat.supports_reasoning_effort => {
			if let Some(Some(off_value)) = mapped_level(model, ModelThinkingLevel::Off) {
				params.insert("reasoning_effort".into(), Value::String(off_value.into()));
			}
		}
		_ => {}
	}

	// Cap reasoning with a top-level budget field. Independent of thinkingFormat: the
	// same server can serve zai, qwen or chat-template models. Reasoning and the answer
	// share max_tokens here, so an uncapped reasoning phase can consume the whole
	// response and leave no answer and no tool call.
	if let (Some(field), Some(thinking_budget)) = (thinking_token_budget_field, thinking_budget) {
		params.insert(
			thinking_token_budget_field_name(field).into(),
			number_value(thinking_budget),
		);
	}

	// OpenRouter provider routing preferences
	let model_compat = model.compat.as_ref();
	if let Some(routing) = model_compat
		.and_then(|compat| compat.get("openRouterRouting"))
		.filter(|routing| is_truthy(routing))
	{
		params.insert("provider".into(), routing.clone());
	}

	// Vercel AI Gateway provider routing preferences
	if let Some(routing) = model_compat
		.and_then(|compat| compat.get("vercelGatewayRouting"))
		.filter(|routing| is_truthy(routing))
	{
		let only = routing.get("only").filter(|only| is_truthy(only));
		let order = routing.get("order").filter(|order| is_truthy(order));
		if only.is_some() || order.is_some() {
			let mut gateway_options = Map::new();
			if let Some(only) = only {
				gateway_options.insert("only".into(), only.clone());
			}
			if let Some(order) = order {
				gateway_options.insert("order".into(), order.clone());
			}
			params.insert("providerOptions".into(), json!({ "gateway": gateway_options }));
		}
	}

	// Last so model and request sampling parameters override named request fields.
	let sampling_params = resolve_sampling_params(
		model,
		effort.map_or(ModelThinkingLevel::Off, ModelThinkingLevel::from),
		options.and_then(|options| options.sampling_params.as_ref()),
	);
	if let Some(sampling_params) = sampling_params {
		for (key, value) in sampling_params {
			unset_keys.retain(|unset_key| *unset_key != key);
			params.insert(key, value);
		}
	}
	for key in unset_keys {
		params.shift_remove(key);
	}

	Ok(Value::Object(params))
}

fn transcript_tool_support(compat: &ResolvedOpenAICompletionsCompat) -> TranscriptToolSupport {
	if compat.supports_mid_convo_system_messages == Some(true) && compat.supports_mid_convo_tool_additions == Some(true)
	{
		TranscriptToolSupport::Additions
	} else {
		TranscriptToolSupport::None
	}
}

fn thinking_token_budget_field_name(field: ThinkingTokenBudgetField) -> &'static str {
	match field {
		ThinkingTokenBudgetField::ThinkingTokenBudget => "thinking_token_budget",
		ThinkingTokenBudgetField::ThinkingBudget => "thinking_budget",
		ThinkingTokenBudgetField::ThinkingBudgetTokens => "thinking_budget_tokens",
	}
}

fn resolve_thinking_token_budget_field(compat: &ResolvedOpenAICompletionsCompat) -> Option<ThinkingTokenBudgetField> {
	if let Some(field) = compat.thinking_token_budget_field {
		return Some(field);
	}
	if compat.supports_thinking_token_budget == Some(true) {
		return Some(ThinkingTokenBudgetField::ThinkingTokenBudget);
	}
	None
}

fn resolve_clamped_thinking_budget(
	model: &Model,
	options: Option<&OpenAICompletionsOptions>,
	params: &Map<String, Value>,
) -> Option<f64> {
	let options = options?;
	let effort = options.reasoning_effort?;
	if !model.reasoning {
		return None;
	}
	let ceiling = ["max_tokens", "max_completion_tokens"]
		.into_iter()
		.find_map(|field| params.get(field).filter(|value| !value.is_null()))
		.map_or(model.max_tokens, |value| value.as_f64().unwrap_or(f64::NAN));
	let budget = clamp_thinking_budget_to_answer_room(
		thinking_budget_for_level(effort, options.thinking_budgets.as_ref()),
		ceiling,
	);
	(budget > 0.0).then_some(budget)
}

fn build_chat_template_values(
	model: &Model,
	reasoning_effort: Option<ThinkingLevel>,
	values: &IndexMap<String, ChatTemplateKwargValue>,
	thinking_budget: Option<f64>,
) -> Option<Map<String, Value>> {
	let mut resolved_values = Map::new();

	for (key, value) in values {
		if let Some(resolved) = resolve_chat_template_kwarg_value(model, reasoning_effort, value, thinking_budget) {
			resolved_values.insert(key.clone(), resolved);
		}
	}

	(!resolved_values.is_empty()).then_some(resolved_values)
}

fn resolve_chat_template_kwarg_value(
	model: &Model,
	reasoning_effort: Option<ThinkingLevel>,
	value: &ChatTemplateKwargValue,
	thinking_budget: Option<f64>,
) -> Option<Value> {
	let variable = match value {
		ChatTemplateKwargValue::String(value) => return Some(Value::String(value.clone())),
		ChatTemplateKwargValue::Number(value) => return Some(number_value(*value)),
		ChatTemplateKwargValue::Bool(value) => return Some(Value::Bool(*value)),
		ChatTemplateKwargValue::Null => return Some(Value::Null),
		ChatTemplateKwargValue::Var(variable) => variable,
	};

	if reasoning_effort.is_none() && variable.omit_when_off() {
		return None;
	}
	match variable.var() {
		Some(ChatTemplateVarName::ThinkingEnabled) => return Some(Value::Bool(reasoning_effort.is_some())),
		Some(ChatTemplateVarName::ThinkingBudget) => return thinking_budget.map(number_value),
		Some(ChatTemplateVarName::ThinkingEffort) | None => {}
	}

	let mapped_value = match reasoning_effort {
		Some(effort) => mapped_level(model, effort.into()),
		None => mapped_level(model, ModelThinkingLevel::Off),
	};
	match mapped_value {
		None => reasoning_effort.map(|effort| Value::String(effort.as_str().into())),
		Some(mapped) => mapped.map(|mapped| Value::String(mapped.into())),
	}
}

fn get_compat_cache_control(
	compat: &ResolvedOpenAICompletionsCompat,
	cache_retention: CacheRetention,
) -> Option<Value> {
	if compat.cache_control_format != Some(CacheControlFormat::Anthropic) || cache_retention == CacheRetention::None {
		return None;
	}

	let mut cache_control = Map::new();
	cache_control.insert("type".into(), Value::String("ephemeral".into()));
	if cache_retention == CacheRetention::Long && compat.supports_long_cache_retention {
		cache_control.insert("ttl".into(), Value::String("1h".into()));
	}
	Some(Value::Object(cache_control))
}

fn apply_anthropic_cache_control(messages: &mut [Value], tools: Option<&mut Vec<Value>>, cache_control: &Value) {
	add_cache_control_to_system_prompt(messages, cache_control);
	add_cache_control_to_last_tool(tools, cache_control);
	add_cache_control_to_last_conversation_message(messages, cache_control);
}

fn message_role(message: &Value) -> Option<&str> {
	message.get("role").and_then(Value::as_str)
}

fn add_cache_control_to_system_prompt(messages: &mut [Value], cache_control: &Value) {
	for message in messages {
		if matches!(message_role(message), Some("system" | "developer")) {
			add_cache_control_to_text_content(message, cache_control);
			return;
		}
	}
}

fn add_cache_control_to_last_conversation_message(messages: &mut [Value], cache_control: &Value) {
	for message in messages.iter_mut().rev() {
		if matches!(message_role(message), Some("user" | "assistant" | "tool"))
			&& add_cache_control_to_text_content(message, cache_control)
		{
			return;
		}
	}
}

fn add_cache_control_to_last_tool(tools: Option<&mut Vec<Value>>, cache_control: &Value) {
	let Some(last_tool) = tools.and_then(|tools| tools.last_mut()) else {
		return;
	};
	if let Some(last_tool) = last_tool.as_object_mut() {
		last_tool.insert("cache_control".into(), cache_control.clone());
	}
}

fn add_cache_control_to_text_content(message: &mut Value, cache_control: &Value) -> bool {
	let Some(message) = message.as_object_mut() else {
		return false;
	};
	match message.get_mut("content") {
		Some(Value::String(content)) => {
			if content.is_empty() {
				return false;
			}
			let text = std::mem::take(content);
			message.insert(
				"content".into(),
				json!([{ "type": "text", "text": text, "cache_control": cache_control }]),
			);
			true
		}
		Some(Value::Array(content)) => {
			for part in content.iter_mut().rev() {
				if part.get("type").and_then(Value::as_str) == Some("text")
					&& let Some(part) = part.as_object_mut()
				{
					part.insert("cache_control".into(), cache_control.clone());
					return true;
				}
			}
			false
		}
		_ => false,
	}
}

/// `convertMessages`: the Chat Completions `messages` of a transcript.
pub fn convert_messages(
	model: &Model,
	context: &TranscriptContext,
	compat: &ResolvedOpenAICompletionsCompat,
	options: Option<ConvertCompletionsMessagesOptions>,
) -> PiResult<Vec<Value>> {
	let normalized_context = resolve_transcript(context, compat.supports_mid_convo_system_messages);
	let mut params: Vec<Value> = Vec::new();
	let grammar_tool_input_properties = options.and_then(|options| options.grammar_tool_input_properties);

	let is_openai = model.provider == "openai";
	let normalize = move |id: &str, _: &Model, _: &AssistantMessage| normalize_tool_call_id(is_openai, id);
	let transformed_messages = transform_messages(&normalized_context.messages, model, Some(&normalize));
	let transcript_tools = resolve_transcript_tools(&normalized_context.messages, transcript_tool_support(compat));
	let instruction_role = if model.reasoning && compat.supports_developer_role {
		"developer"
	} else {
		"system"
	};

	let mut last_role: Option<&str> = None;
	let mut system_index = 0;

	let mut i = 0;
	while i < transformed_messages.len() {
		let msg = &transformed_messages[i];
		// Some providers don't allow user messages directly after tool results
		// Insert a synthetic assistant message to bridge the gap
		if compat.requires_assistant_after_tool_result
			&& last_role == Some("toolResult")
			&& matches!(msg, Message::User(_))
		{
			params.push(json!({ "role": "assistant", "content": "I have processed the tool results." }));
		}

		match msg {
			Message::System(msg) => {
				// transformMessages keeps system messages in order, so their position matches the transcript.
				let folded = system_index < transcript_tools.folded_system_messages;
				system_index += 1;
				let added_tools: &[Tool] = if folded {
					&[]
				} else {
					msg.tools_added.as_deref().unwrap_or_default()
				};
				if !added_tools.is_empty() {
					params.push(json!({ "role": "system", "tools": convert_tools(added_tools, compat)? }));
				}
				let text = if i == 0 {
					get_system_message_text(msg)
				} else {
					render_system_message_update(msg)
				};
				if !text.is_empty() {
					params.push(json!({ "role": instruction_role, "content": sanitize_surrogates(&text) }));
				}
				last_role = Some("system");
			}
			Message::User(msg) => {
				match &msg.content {
					UserMessageContent::Text(content) => {
						params.push(json!({ "role": "user", "content": sanitize_surrogates(content) }));
					}
					UserMessageContent::Blocks(blocks) => {
						let content: Vec<Value> = blocks
							.iter()
							.filter(|item| !matches!(item, UserContent::Text(text) if text.text.is_empty()))
							.map(|item| match item {
								UserContent::Text(item) => {
									json!({ "type": "text", "text": sanitize_surrogates(&item.text) })
								}
								UserContent::Image(item) => json!({
									"type": "image_url",
									"image_url": { "url": format!("data:{};base64,{}", item.mime_type, item.data) },
								}),
							})
							.collect();
						if content.is_empty() {
							i += 1;
							continue;
						}
						params.push(json!({ "role": "user", "content": content }));
					}
				}
				last_role = Some("user");
			}
			Message::Assistant(msg) => {
				let Some(assistant_msg) = convert_assistant_message(model, msg, compat, grammar_tool_input_properties)?
				else {
					i += 1;
					continue;
				};
				params.push(assistant_msg);
				last_role = Some("assistant");
			}
			Message::ToolResult(_) => {
				let mut image_blocks: Vec<Value> = Vec::new();
				let mut j = i;

				while let Some(Message::ToolResult(tool_msg)) = transformed_messages.get(j) {
					// Extract text and image content
					let text_result = tool_msg
						.content
						.iter()
						.filter_map(|block| match block {
							UserContent::Text(block) => Some(block.text.as_str()),
							UserContent::Image(_) => None,
						})
						.collect::<Vec<_>>()
						.join("\n");
					let has_images = tool_msg
						.content
						.iter()
						.any(|block| matches!(block, UserContent::Image(_)));

					// Always send tool result with text (or placeholder if only images)
					let has_text = !js_trim(&text_result).is_empty();
					let tool_result_text = if has_text {
						text_result.as_str()
					} else if has_images {
						"(see attached image)"
					} else {
						"(no tool output)"
					};
					// Some providers require the 'name' field in tool results
					let mut tool_result_msg = Map::new();
					tool_result_msg.insert("role".into(), Value::String("tool".into()));
					tool_result_msg.insert(
						"content".into(),
						Value::String(sanitize_surrogates(tool_result_text).into()),
					);
					tool_result_msg.insert("tool_call_id".into(), Value::String(tool_msg.tool_call_id.clone()));
					if compat.requires_tool_result_name && !tool_msg.tool_name.is_empty() {
						tool_result_msg.insert("name".into(), Value::String(tool_msg.tool_name.clone()));
					}
					params.push(Value::Object(tool_result_msg));

					if has_images && model.input.contains(&ModelInputModality::Image) {
						for block in &tool_msg.content {
							if let UserContent::Image(block) = block {
								image_blocks.push(json!({
									"type": "image_url",
									"image_url": { "url": format!("data:{};base64,{}", block.mime_type, block.data) },
								}));
							}
						}
					}
					j += 1;
				}

				i = j;

				if image_blocks.is_empty() {
					last_role = Some("toolResult");
				} else {
					if compat.requires_assistant_after_tool_result {
						params.push(json!({ "role": "assistant", "content": "I have processed the tool results." }));
					}

					let mut content = vec![json!({ "type": "text", "text": "Attached image(s) from tool result:" })];
					content.extend(image_blocks);
					params.push(json!({ "role": "user", "content": content }));
					last_role = Some("user");
				}

				continue;
			}
		}
		i += 1;
	}

	Ok(params)
}

/// One assistant message of `convertMessages`, or `None` when it has no content and no tool calls.
fn convert_assistant_message(
	model: &Model,
	msg: &AssistantMessage,
	compat: &ResolvedOpenAICompletionsCompat,
	grammar_tool_input_properties: Option<&IndexMap<String, String>>,
) -> PiResult<Option<Value>> {
	// Some providers don't accept null content, use empty string instead
	let mut assistant_msg = Map::new();
	assistant_msg.insert("role".into(), Value::String("assistant".into()));
	assistant_msg.insert(
		"content".into(),
		if compat.requires_assistant_after_tool_result {
			Value::String(String::new())
		} else {
			Value::Null
		},
	);

	let assistant_text_parts: Vec<String> = msg
		.content
		.iter()
		.filter_map(|block| match block {
			AssistantContent::Text(block) if !js_trim(&block.text).is_empty() => {
				Some(sanitize_surrogates(&block.text).to_string())
			}
			_ => None,
		})
		.collect();
	let assistant_text = assistant_text_parts.join("");

	let thinking_blocks: Vec<&ThinkingContent> = msg
		.content
		.iter()
		.filter_map(|block| match block {
			AssistantContent::Thinking(block) => Some(block),
			_ => None,
		})
		.collect();
	let tool_calls: Vec<&ToolCall> = msg
		.content
		.iter()
		.filter_map(|block| match block {
			AssistantContent::ToolCall(block) => Some(block),
			_ => None,
		})
		.collect();
	let signed_reasoning_details = thinking_blocks
		.iter()
		.find_map(|block| parse_openai_reasoning_details(block.thinking_signature.as_deref()));
	let legacy_reasoning_details: Vec<Value> = tool_calls
		.iter()
		.filter_map(|tool_call| parse_legacy_encrypted_reasoning_detail(tool_call.thought_signature.as_deref()))
		.collect();
	let preserved_reasoning_details =
		signed_reasoning_details.or_else(|| (!legacy_reasoning_details.is_empty()).then_some(legacy_reasoning_details));

	let non_empty_thinking_blocks: Vec<&ThinkingContent> = thinking_blocks
		.into_iter()
		.filter(|block| !js_trim(&block.thinking).is_empty())
		.collect();
	if !non_empty_thinking_blocks.is_empty() {
		if compat.requires_thinking_as_text {
			// Convert thinking blocks to plain text (no tags to avoid model mimicking them)
			let thinking_text = non_empty_thinking_blocks
				.iter()
				.map(|block| sanitize_surrogates(&block.thinking))
				.collect::<Vec<_>>()
				.join("\n\n");
			let mut content = vec![json!({ "type": "text", "text": thinking_text })];
			content.extend(
				assistant_text_parts
					.iter()
					.map(|text| json!({ "type": "text", "text": text })),
			);
			assistant_msg.insert("content".into(), Value::Array(content));
		} else {
			// Always send assistant content as a plain string (OpenAI Chat Completions
			// API standard format). Sending as an array of {type:"text", text:"..."}
			// objects is non-standard and causes some models (e.g. DeepSeek V3.2 via
			// NVIDIA NIM) to mirror the content-block structure literally in their
			// output, producing recursive nesting like [{'type':'text','text':'[{...}]'}].
			if !assistant_text.is_empty() {
				assistant_msg.insert("content".into(), Value::String(assistant_text.clone()));
			}

			// reasoning_details is the structured alternative to a raw reasoning field.
			if preserved_reasoning_details.is_none() {
				// Use the signature from the first thinking block if available (for llama.cpp server + gpt-oss)
				let mut signature = non_empty_thinking_blocks[0].thinking_signature.as_deref();
				if model.provider == "opencode-go" && signature == Some("reasoning") {
					signature = Some("reasoning_content");
				}
				if let Some(signature) = signature.filter(|signature| is_openai_completions_reasoning_field(signature))
				{
					let thinking = non_empty_thinking_blocks
						.iter()
						.map(|block| block.thinking.as_str())
						.collect::<Vec<_>>()
						.join("\n");
					assistant_msg.insert(signature.into(), Value::String(thinking));
				}
			}
		}
	} else if !assistant_text.is_empty() {
		// Always send assistant content as a plain string (OpenAI Chat Completions
		// API standard format). Sending as an array of {type:"text", text:"..."}
		// objects is non-standard and causes some models (e.g. DeepSeek V3.2 via
		// NVIDIA NIM) to mirror the content-block structure literally in their
		// output, producing recursive nesting like [{'type':'text','text':'[{...}]'}].
		assistant_msg.insert("content".into(), Value::String(assistant_text));
	}

	if !tool_calls.is_empty() {
		let tool_calls = tool_calls
			.iter()
			.map(|tc| {
				if let Some(custom_input_property) =
					grammar_tool_input_properties.and_then(|properties| properties.get(&tc.name))
				{
					let input = get_grammar_tool_input(&tc.name, &tc.arguments, custom_input_property)?;
					return Ok(json!({
						"id": tc.id,
						"type": "custom",
						"custom": { "name": tc.name, "input": sanitize_surrogates(&input) },
					}));
				}
				Ok(json!({
					"id": tc.id,
					"type": "function",
					"function": { "name": tc.name, "arguments": json_stringify(&tc.arguments) },
				}))
			})
			.collect::<PiResult<Vec<Value>>>()?;
		assistant_msg.insert("tool_calls".into(), Value::Array(tool_calls));
	}
	if let Some(preserved_reasoning_details) = preserved_reasoning_details {
		assistant_msg.insert("reasoning_details".into(), Value::Array(preserved_reasoning_details));
	}
	if compat.requires_reasoning_content_on_assistant_messages
		&& model.reasoning
		&& !assistant_msg.contains_key("reasoning_content")
	{
		assistant_msg.insert("reasoning_content".into(), Value::String(String::new()));
	}
	// Skip assistant messages that have no content and no tool calls.
	// Some providers require "either content or tool_calls, but not none".
	// Other providers also don't accept empty assistant messages.
	// This handles aborted assistant responses that got no content.
	let has_content = match assistant_msg.get("content") {
		Some(Value::String(content)) => !content.is_empty(),
		Some(Value::Array(content)) => !content.is_empty(),
		_ => false,
	};
	if !has_content && !assistant_msg.contains_key("tool_calls") {
		return Ok(None);
	}
	Ok(Some(Value::Object(assistant_msg)))
}

/// Characters outside `[a-zA-Z0-9_-]` become `_`, one per UTF-16 code unit as the TS regex replaces them.
fn sanitize_tool_call_id_part(text: &str) -> String {
	let mut out = String::with_capacity(text.len());
	for character in text.chars() {
		if character.is_ascii_alphanumeric() || character == '_' || character == '-' {
			out.push(character);
		} else {
			for _ in 0..character.len_utf16() {
				out.push('_');
			}
		}
	}
	out
}

fn normalize_tool_call_id(is_openai: bool, id: &str) -> String {
	// Handle pipe-separated IDs from OpenAI Responses API
	// Format: {call_id}|{id} where {id} can be 400+ chars with special chars (+, /, =)
	// These come from providers like github-copilot, openai-codex, opencode
	// Extract just the call_id part and normalize it
	// Multiple tool calls in the same turn can share call_id but differ by item_id.
	// Preserve item-level uniqueness when replaying into Chat Completions, which
	// requires distinct tool call ids.
	if let Some(separator_index) = id.find('|') {
		// Sanitize to allowed chars and truncate to 40 chars (OpenAI limit)
		let call_id = sanitize_tool_call_id_part(&id[..separator_index]);
		let item_id = sanitize_tool_call_id_part(&id[separator_index + 1..]);
		let combined_id = if item_id.is_empty() {
			call_id.clone()
		} else {
			format!("{call_id}_{item_id}")
		};
		if combined_id.len() <= 40 {
			return combined_id;
		}
		let hash: String = short_hash(id).chars().take(8).collect();
		let prefix_length = 1.max(40 - hash.len() as i64 - 1) as usize;
		let prefix = &call_id[..prefix_length.min(call_id.len())];
		return format!("{prefix}_{hash}");
	}

	if is_openai && utf16_len(id) > 40 {
		return utf16_prefix(id, 40).to_string();
	}
	id.to_string()
}

fn convert_tools(tools: &[Tool], compat: &ResolvedOpenAICompletionsCompat) -> PiResult<Vec<Value>> {
	tools
		.iter()
		.map(|tool| {
			if let Some(grammar) = resolve_grammar_constrained_sampling(tool, compat.supports_openai_grammar_tools)? {
				return Ok(json!({
					"type": "custom",
					"custom": {
						"name": tool.name,
						"description": tool.description,
						"format": {
							"type": "grammar",
							"grammar": { "syntax": grammar.format.as_str(), "definition": grammar.definition },
						},
					},
				}));
			}

			let strict = resolve_json_schema_strict_sampling(tool, compat.supports_strict_mode, None)?;
			let mut function = Map::new();
			function.insert("name".into(), Value::String(tool.name.clone()));
			function.insert("description".into(), Value::String(tool.description.clone()));
			function.insert("parameters".into(), get_json_schema_tool_parameters(tool, strict)?);
			// Only include strict if provider supports it. Some reject unknown fields.
			if compat.supports_strict_mode {
				function.insert("strict".into(), Value::Bool(strict.unwrap_or(false)));
			}
			Ok(json!({ "type": "function", "function": function }))
		})
		.collect()
}

/// `value || 0` for a usage count.
fn count_or_zero(value: Option<&Value>) -> f64 {
	value
		.and_then(Value::as_f64)
		.filter(|count| *count != 0.0)
		.unwrap_or(0.0)
}

fn parse_chunk_usage(
	// OpenRouter adds billing fields to streamed usage at runtime,
	// although the OpenAI SDK does not type those fields.
	raw_usage: &Value,
	model: &Model,
) -> Usage {
	let details = raw_usage.get("prompt_tokens_details");
	let prompt_tokens = count_or_zero(raw_usage.get("prompt_tokens"));
	let cache_read_tokens = [
		present(details, "cached_tokens"),
		present(Some(raw_usage), "prompt_cache_hit_tokens"),
		present(Some(raw_usage), "cached_tokens"),
	]
	.into_iter()
	.flatten()
	.next()
	.map_or(0.0, |count| count.as_f64().unwrap_or(0.0));
	let cache_write_tokens = count_or_zero(details.and_then(|details| details.get("cache_write_tokens")));

	// Follow documented OpenAI/OpenRouter semantics: cached_tokens is cache-read
	// tokens (hits). Providers disagree on placement: OpenAI/OpenRouter use
	// prompt_tokens_details.cached_tokens, DeepSeek uses prompt_cache_hit_tokens,
	// and Kimi documents top-level usage.cached_tokens on the final usage chunk.
	// OpenAI does not document or emit cache_write_tokens, but
	// OpenRouter-compatible providers can include it as a separate write count.
	// OpenRouter's own provider/tests affirm the separate mapping:
	// https://github.com/OpenRouterTeam/ai-sdk-provider/pull/409
	// Do not subtract writes from cached_tokens, otherwise spec-compliant
	// providers are under-reported. DS4 mirrors this contract too:
	// https://github.com/antirez/ds4/pull/29
	let input = pi_host::js::math_max(0.0, prompt_tokens - cache_read_tokens - cache_write_tokens);
	// OpenAI completion_tokens already includes reasoning_tokens.
	let output_tokens = count_or_zero(raw_usage.get("completion_tokens"));
	let mut usage = Usage {
		input,
		output: output_tokens,
		cache_read: cache_read_tokens,
		cache_write: cache_write_tokens,
		reasoning: Some(count_or_zero(
			raw_usage
				.get("completion_tokens_details")
				.and_then(|details| details.get("reasoning_tokens")),
		)),
		total_tokens: input + output_tokens + cache_read_tokens + cache_write_tokens,
		..Usage::default()
	};
	calculate_cost(model, &mut usage);
	if is_open_router_model(&model.base) {
		apply_open_router_reported_cost(&mut usage, raw_usage);
	}
	usage
}

fn map_stop_reason(reason: &str) -> (StopReason, Option<String>) {
	match reason {
		"stop" | "end" => (StopReason::Stop, None),
		"length" => (StopReason::Length, None),
		"function_call" | "tool_calls" => (StopReason::ToolUse, None),
		"content_filter" => (StopReason::Error, Some("Provider finish_reason: content_filter".into())),
		"network_error" => (StopReason::Error, Some("Provider finish_reason: network_error".into())),
		reason => (StopReason::Error, Some(format!("Provider finish_reason: {reason}"))),
	}
}

/// Auto-detect compatibility settings from provider name and baseUrl.
/// Used as the base when model.compat is not set; explicit model.compat
/// entries override these detected values.
fn detect_compat(model: &Model) -> ResolvedOpenAICompletionsCompat {
	let provider: &str = &model.provider;
	let hostname = url_hostname(&model.base_url).unwrap_or_default();
	let hostname = hostname.as_str();
	let matches_domain = |domain: &str| hostname == domain || hostname.ends_with(&format!(".{domain}"));

	let is_zai =
		provider == "zai" || provider == "zai-coding-cn" || hostname == "api.z.ai" || hostname == "open.bigmodel.cn";
	let is_together = provider == "together" || hostname == "api.together.ai" || hostname == "api.together.xyz";
	let is_moonshot = provider == "moonshotai"
		|| provider == "moonshotai-cn"
		|| hostname == "api.moonshot.ai"
		|| hostname == "api.moonshot.cn";
	let is_open_router = provider == "openrouter" || matches_domain("openrouter.ai");
	let is_cloudflare_workers_ai = provider == "cloudflare-workers-ai" || hostname == "api.cloudflare.com";
	let is_cloudflare_ai_gateway = provider == "cloudflare-ai-gateway" || hostname == "gateway.ai.cloudflare.com";
	let is_nvidia = provider == "nvidia" || hostname == "integrate.api.nvidia.com";
	let is_ant_ling = provider == "ant-ling" || hostname == "api.ant-ling.com";
	let is_cerebras = provider == "cerebras" || matches_domain("cerebras.ai");
	let is_deep_seek = provider == "deepseek" || matches_domain("deepseek.com");

	let is_non_standard = is_nvidia
		|| is_cerebras
		|| provider == "xai"
		|| hostname == "api.x.ai"
		|| is_together
		|| matches_domain("chutes.ai")
		|| is_deep_seek
		|| is_zai
		|| is_moonshot
		|| provider == "opencode"
		|| matches_domain("opencode.ai")
		|| is_cloudflare_workers_ai
		|| is_cloudflare_ai_gateway
		|| is_ant_ling;

	let use_max_tokens = matches_domain("chutes.ai")
		|| is_deep_seek
		|| is_moonshot
		|| is_cloudflare_ai_gateway
		|| is_together
		|| is_nvidia
		|| is_ant_ling
		|| is_zai;

	let is_grok = provider == "xai" || hostname == "api.x.ai";
	let is_open_router_developer_role_model =
		is_open_router && (model.id.starts_with("anthropic/") || model.id.starts_with("openai/"));
	let cache_control_format =
		(provider == "openrouter" && model.id.starts_with("anthropic/")).then_some(CacheControlFormat::Anthropic);

	ResolvedOpenAICompletionsCompat {
		supports_store: !is_non_standard,
		supports_developer_role: is_open_router_developer_role_model || (!is_non_standard && !is_open_router),
		supports_reasoning_effort: !is_grok
			&& !is_zai
			&& !is_moonshot
			&& !is_together
			&& !is_cloudflare_ai_gateway
			&& !is_nvidia
			&& !is_ant_ling,
		supports_usage_in_streaming: true,
		supports_finish_reason: true,
		max_tokens_field: if use_max_tokens {
			MaxTokensField::MaxTokens
		} else {
			MaxTokensField::MaxCompletionTokens
		},
		requires_tool_result_name: false,
		requires_assistant_after_tool_result: false,
		requires_thinking_as_text: false,
		requires_reasoning_content_on_assistant_messages: is_deep_seek,
		thinking_format: if is_deep_seek {
			ThinkingFormat::Deepseek
		} else if is_zai {
			ThinkingFormat::Zai
		} else if is_together {
			ThinkingFormat::Together
		} else if is_ant_ling {
			ThinkingFormat::AntLing
		} else if is_open_router {
			ThinkingFormat::Openrouter
		} else {
			ThinkingFormat::Openai
		},
		open_router_routing: json!({}),
		vercel_gateway_routing: json!({}),
		chat_template_kwargs: IndexMap::new(),
		chat_template_args: IndexMap::new(),
		zai_tool_stream: false,
		supports_thinking_token_budget: Some(false),
		thinking_token_budget_field: None,
		// OpenAI compatibility alone does not imply strict JSON-schema tool support.
		supports_strict_mode: false,
		supports_openai_grammar_tools: false,
		supports_mid_convo_system_messages: Some(false),
		supports_mid_convo_tool_additions: Some(false),
		cache_control_format,
		send_session_affinity_headers: is_open_router,
		session_affinity_format: if is_open_router {
			SessionAffinityFormat::Openrouter
		} else {
			SessionAffinityFormat::Openai
		},
		supports_long_cache_retention: !(is_together
			|| is_cloudflare_workers_ai
			|| is_cloudflare_ai_gateway
			|| is_nvidia
			|| is_ant_ling),
		vllm_priority: None,
	}
}

/// Get resolved compatibility settings for a model.
/// Auto-detects from provider/URL then overrides with explicit model.compat.
pub fn get_compat(model: &Model) -> ResolvedOpenAICompletionsCompat {
	let detected = detect_compat(model);
	let Some(raw_compat) = model.compat.as_ref() else {
		return detected;
	};
	let compat = model.openai_completions_compat();
	let raw = |key: &str| raw_compat.get(key).filter(|value| !value.is_null()).cloned();

	ResolvedOpenAICompletionsCompat {
		supports_store: compat.supports_store.unwrap_or(detected.supports_store),
		supports_developer_role: compat
			.supports_developer_role
			.unwrap_or(detected.supports_developer_role),
		supports_reasoning_effort: compat
			.supports_reasoning_effort
			.unwrap_or(detected.supports_reasoning_effort),
		supports_usage_in_streaming: compat
			.supports_usage_in_streaming
			.unwrap_or(detected.supports_usage_in_streaming),
		supports_finish_reason: compat.supports_finish_reason.unwrap_or(detected.supports_finish_reason),
		max_tokens_field: compat.max_tokens_field.unwrap_or(detected.max_tokens_field),
		requires_tool_result_name: compat
			.requires_tool_result_name
			.unwrap_or(detected.requires_tool_result_name),
		requires_assistant_after_tool_result: compat
			.requires_assistant_after_tool_result
			.unwrap_or(detected.requires_assistant_after_tool_result),
		requires_thinking_as_text: compat
			.requires_thinking_as_text
			.unwrap_or(detected.requires_thinking_as_text),
		requires_reasoning_content_on_assistant_messages: compat
			.requires_reasoning_content_on_assistant_messages
			.unwrap_or(detected.requires_reasoning_content_on_assistant_messages),
		thinking_format: compat.thinking_format.unwrap_or(detected.thinking_format),
		open_router_routing: raw("openRouterRouting").unwrap_or_else(|| json!({})),
		vercel_gateway_routing: raw("vercelGatewayRouting").unwrap_or(detected.vercel_gateway_routing),
		chat_template_kwargs: compat.chat_template_kwargs.unwrap_or(detected.chat_template_kwargs),
		chat_template_args: compat.chat_template_args.unwrap_or(detected.chat_template_args),
		zai_tool_stream: compat.zai_tool_stream.unwrap_or(detected.zai_tool_stream),
		supports_thinking_token_budget: compat
			.supports_thinking_token_budget
			.or(detected.supports_thinking_token_budget),
		thinking_token_budget_field: compat
			.thinking_token_budget_field
			.or(detected.thinking_token_budget_field),
		supports_strict_mode: compat.supports_strict_mode.unwrap_or(detected.supports_strict_mode),
		supports_openai_grammar_tools: compat
			.supports_openai_grammar_tools
			.unwrap_or(detected.supports_openai_grammar_tools),
		supports_mid_convo_system_messages: compat
			.supports_mid_convo_system_messages
			.or(detected.supports_mid_convo_system_messages),
		supports_mid_convo_tool_additions: compat
			.supports_mid_convo_tool_additions
			.or(detected.supports_mid_convo_tool_additions),
		cache_control_format: compat.cache_control_format.or(detected.cache_control_format),
		send_session_affinity_headers: compat
			.send_session_affinity_headers
			.unwrap_or(detected.send_session_affinity_headers),
		session_affinity_format: compat
			.session_affinity_format
			.unwrap_or(detected.session_affinity_format),
		supports_long_cache_retention: compat
			.supports_long_cache_retention
			.unwrap_or(detected.supports_long_cache_retention),
		vllm_priority: compat.vllm_priority,
	}
}
