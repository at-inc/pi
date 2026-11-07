//! Port of `packages/ai/src/api/google-generative-ai.ts`.
//! PORT-STATUS: ported
//!
//! The `@google/genai` SDK is `api/google_client.rs`. Differences from the TS:
//! - `GoogleOptions` read from `ProviderStreamOptions.extra`: `toolChoice` is read when it is a string and
//!   `thinking` when it is an object. `thinking.enabled` follows JS truthiness. A `thinking.level` that is present but
//!   outside `GoogleApiThinkingLevel` sends `thinkingConfig` without a level and ignores `budgetTokens`, as in the TS.
//!   A `budgetTokens` that is not a number is omitted, where the TS sends the value as is.
//! - Stream chunks are the SDK's `GenerateContentResponse` as JSON and are read with the wire types: a part `text`,
//!   `thoughtSignature`, `finishReason` or `responseId` that is not a string is ignored, function call `args` that are
//!   not an object read as `{}`, and a non-numeric usage count reads as 0.
//! - The abort signal is passed to the client next to the params, so it still applies when `onPayload` replaces the
//!   params (the TS signal lives in `params.config.abortSignal`). The payload `onPayload` receives has no
//!   `config.abortSignal` key, which `JSON.stringify` shows as `{}` in the TS.
//! - The current text or thinking block is always the last content block, so it is tracked by kind, not by object.
//! - `google-vertex.ts` repeats this module's stream loop and `buildParams`; the Rust `google_vertex` calls
//!   `stream_google_response` and `build_params` here instead, keeping its own tool call counter and messages.

use std::cell::RefCell;
use std::ops::{Deref, DerefMut};
use std::rc::Rc;
use std::sync::atomic::{AtomicU64, Ordering};

use futures::StreamExt;
use indexmap::IndexMap;
use pi_host::js::{json_stringify, number_to_string};
use pi_host::{PiError, PiResult};
use serde_json::{Map, Value, json};

use crate::api::google_client::{
	GenerateContentStream, GoogleGenAI, GoogleGenAIOptions, HttpOptions, js_string, js_truthy,
};
use crate::api::google_shared::{
	GoogleApiThinkingLevel, GoogleRetryOptions, ResolvedGoogleThinkingLevel, convert_messages, convert_tools,
	get_disabled_google_thinking_config, is_thinking_part, map_stop_reason, resolve_google_function_calling_mode,
	resolve_google_thinking_level, retain_thought_signature, retry_google_request,
	supports_google_strict_tool_sampling, to_google_sdk_thinking_level, to_google_thinking_level,
	uses_google_thinking_level,
};
use crate::api::simple_options::build_base_options;
use crate::models::{calculate_cost, clamp_thinking_level};
use crate::types::{
	AssistantContent, AssistantMessage, AssistantMessageEvent, Model, ModelThinkingLevel, ProviderHeaders,
	ProviderStreamOptions, ProviderStreams, SharedAssistantMessage, SimpleStreamOptions, StopReason, StreamOptions,
	TextContent, ThinkingBudgets, ThinkingContent, ToolCall, ToolChoice, TranscriptContext, Usage, api,
};
use crate::utils::error_body::{format_provider_error, normalize_provider_error};
use crate::utils::event_stream::AssistantMessageEventStream;
use crate::utils::headers::provider_headers_to_record;
use crate::utils::identity::{ClientIdentity, ClientIdentityTarget, get_identity_headers};
use crate::utils::sanitize_unicode::sanitize_surrogates;
use crate::utils::text::get_system_message_text;
use crate::utils::transcript::{collapse_system_messages, get_current_tools, get_initial_system_message};

/// `GoogleOptions.thinking`.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct GoogleThinkingOptions {
	pub enabled: bool,
	/// -1 for dynamic, 0 to disable
	pub budget_tokens: Option<f64>,
	pub level: Option<GoogleApiThinkingLevel>,
}

/// `GoogleOptions`.
#[derive(Clone, Default)]
pub struct GoogleOptions {
	pub base: StreamOptions,
	/// `"auto" | "none" | "any"`.
	pub tool_choice: Option<String>,
	pub thinking: Option<GoogleThinkingOptions>,
}

impl Deref for GoogleOptions {
	type Target = StreamOptions;

	fn deref(&self) -> &Self::Target {
		&self.base
	}
}

impl DerefMut for GoogleOptions {
	fn deref_mut(&mut self) -> &mut Self::Target {
		&mut self.base
	}
}

impl From<ProviderStreamOptions> for GoogleOptions {
	/// Reads `toolChoice` and `thinking` from the extra keys.
	fn from(options: ProviderStreamOptions) -> Self {
		let extra = options.extra;
		Self {
			base: options.base,
			tool_choice: extra.get("toolChoice").and_then(Value::as_str).map(str::to_string),
			thinking: extra
				.get("thinking")
				.and_then(Value::as_object)
				.map(thinking_options_from_json),
		}
	}
}

/// `GoogleOptions.thinking` from JSON, shared with the Vertex options.
pub(crate) fn thinking_options_from_json(thinking: &Map<String, Value>) -> GoogleThinkingOptions {
	// `buildParams` branches on `level !== undefined`: a present level, valid or not, means the budget is ignored.
	let level_present = thinking.contains_key("level");
	GoogleThinkingOptions {
		enabled: thinking.get("enabled").is_some_and(js_truthy),
		budget_tokens: thinking
			.get("budgetTokens")
			.and_then(Value::as_f64)
			.filter(|_| !level_present),
		level: thinking
			.get("level")
			.and_then(Value::as_str)
			.and_then(|level| match level {
				"THINKING_LEVEL_UNSPECIFIED" => Some(GoogleApiThinkingLevel::ThinkingLevelUnspecified),
				"MINIMAL" => Some(GoogleApiThinkingLevel::Minimal),
				"LOW" => Some(GoogleApiThinkingLevel::Low),
				"MEDIUM" => Some(GoogleApiThinkingLevel::Medium),
				"HIGH" => Some(GoogleApiThinkingLevel::High),
				_ => None,
			}),
	}
}

// Counter for generating unique tool call IDs
static TOOL_CALL_COUNTER: AtomicU64 = AtomicU64::new(0);

/// `stream`: streams one Gemini API `generateContentStream` response.
pub fn stream(
	model: &Model,
	context: &TranscriptContext,
	options: Option<GoogleOptions>,
) -> AssistantMessageEventStream {
	let stream = AssistantMessageEventStream::new();
	let normalized_context = collapse_system_messages(context);
	let output: SharedAssistantMessage = Rc::new(RefCell::new(AssistantMessage {
		content: Vec::new(),
		api: api::GOOGLE_GENERATIVE_AI.into(),
		provider: model.provider.clone(),
		model: model.id.clone(),
		usage: Usage::default(),
		stop_reason: StopReason::Pending,
		timestamp: pi_host::now_ms(),
		..AssistantMessage::default()
	}));
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
			message.error_message = Some(format_provider_error(&normalize_provider_error(&error), None));
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
	options: &GoogleOptions,
	output: &SharedAssistantMessage,
) -> PiResult<()> {
	if options
		.fetch
		.as_ref()
		.is_some_and(|fetch| !pi_host::is_global_fetch(fetch))
	{
		return Err(PiError::new(
			"Custom fetch is not supported by the Google Generative AI adapter",
		));
	}
	let Some(api_key) = options.api_key.as_deref().filter(|api_key| !api_key.is_empty()) else {
		return Err(PiError::new(format!("No API key for provider: {}", &*model.provider)));
	};
	let client = create_client(
		model,
		Some(api_key),
		options.headers.as_ref(),
		options.identity.as_ref(),
	)?;
	let mut params = build_params(model, context, options)?;
	if let Some(on_payload) = &options.on_payload
		&& let Some(next_params) = on_payload(params.clone(), model).await?
	{
		params = next_params;
	}
	let google_stream = retry_google_request(
		|| client.generate_content_stream(params.clone(), options.signal.clone()),
		Some(GoogleRetryOptions {
			max_retries: options.max_retries,
			max_retry_delay_ms: options.max_retry_delay_ms,
			signal: options.signal.clone(),
		}),
	)
	.await?;
	stream_google_response(
		stream,
		model,
		options,
		output,
		google_stream,
		&TOOL_CALL_COUNTER,
		"Google stream ended without a finish reason",
	)
	.await
}

/// The part of the `stream` `try` block after the request: pushes `start`, the content events and `done`, or fails
/// with the error the TS throws. Shared with `google_vertex`, which differs only in the missing-finish-reason message
/// and its own tool call counter.
pub(crate) async fn stream_google_response(
	stream: &AssistantMessageEventStream,
	model: &Model,
	options: &StreamOptions,
	output: &SharedAssistantMessage,
	mut google_stream: GenerateContentStream,
	tool_call_counter: &AtomicU64,
	missing_finish_reason: &str,
) -> PiResult<()> {
	stream.push(AssistantMessageEvent::Start {
		partial: output.clone(),
	});
	let mut blocks = GoogleStreamBlocks {
		stream,
		output,
		tool_call_counter,
		current_block: None,
	};
	while let Some(chunk) = google_stream.next().await {
		let chunk = chunk?;
		if let Some(on_provider_stream_event) = &options.on_provider_stream_event {
			on_provider_stream_event(&chunk, model).await?;
		}
		blocks.apply_chunk(model, &chunk)?;
	}
	blocks.end_current_block();

	if options.signal.as_ref().is_some_and(|signal| signal.aborted()) {
		return Err(PiError::new("Request was aborted"));
	}

	let (stop_reason, raw_stop_reason) = {
		let message = output.borrow();
		(message.stop_reason, message.raw_stop_reason.clone())
	};
	if stop_reason == StopReason::Pending {
		return Err(PiError::new(missing_finish_reason));
	}
	if stop_reason == StopReason::Aborted || stop_reason == StopReason::Error {
		let error_message = match raw_stop_reason.filter(|reason| !reason.is_empty()) {
			Some(reason) => format!("Provider stopped with: {reason}"),
			None => "An unknown error occurred".into(),
		};
		return Err(PiError::new(error_message));
	}

	stream.push(AssistantMessageEvent::Done {
		reason: stop_reason,
		message: output.clone(),
	});
	stream.end(None);
	Ok(())
}

/// `currentBlock`'s type. The block itself is the last element of `output.content`.
#[derive(Clone, Copy, PartialEq, Eq)]
enum CurrentBlock {
	Text,
	Thinking,
}

/// The block bookkeeping of one streamed response.
struct GoogleStreamBlocks<'a> {
	stream: &'a AssistantMessageEventStream,
	output: &'a SharedAssistantMessage,
	tool_call_counter: &'a AtomicU64,
	current_block: Option<CurrentBlock>,
}

impl GoogleStreamBlocks<'_> {
	fn block_index(&self) -> usize {
		self.output.borrow().content.len() - 1
	}

	/// Pushes `text_end` or `thinking_end` for the current block and clears it.
	fn end_current_block(&mut self) {
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

	fn apply_chunk(&mut self, model: &Model, chunk: &Value) -> PiResult<()> {
		{
			// @google/genai documents GenerateContentResponse.responseId as an output-only field
			// used to identify each response. Keep the first non-empty one from the stream.
			let mut message = self.output.borrow_mut();
			if message.response_id.as_deref().is_none_or(str::is_empty) {
				message.response_id = chunk.get("responseId").and_then(Value::as_str).map(str::to_string);
			}
		}
		let candidate = chunk
			.get("candidates")
			.and_then(Value::as_array)
			.and_then(|candidates| candidates.first());
		let parts = candidate
			.and_then(|candidate| candidate.get("content"))
			.and_then(|content| content.get("parts"))
			.and_then(Value::as_array);
		for part in parts.into_iter().flatten() {
			if let Some(text) = part.get("text").and_then(Value::as_str) {
				self.apply_text(part, text);
			}
			if let Some(function_call) = part.get("functionCall").and_then(Value::as_object) {
				self.apply_function_call(part, function_call);
			}
		}

		if let Some(finish_reason) = candidate
			.and_then(|candidate| candidate.get("finishReason"))
			.and_then(Value::as_str)
			.filter(|reason| !reason.is_empty())
		{
			let mut message = self.output.borrow_mut();
			message.raw_stop_reason = Some(finish_reason.to_string());
			message.stop_reason = map_stop_reason(finish_reason)?;
			if message.stop_reason == StopReason::Stop
				&& message
					.content
					.iter()
					.any(|block| matches!(block, AssistantContent::ToolCall(_)))
			{
				message.stop_reason = StopReason::ToolUse;
			}
		}

		if let Some(usage_metadata) = chunk.get("usageMetadata").and_then(Value::as_object) {
			let count = |name: &str| {
				usage_metadata
					.get(name)
					.and_then(Value::as_f64)
					.filter(|count| *count != 0.0 && !count.is_nan())
					.unwrap_or(0.0)
			};
			let mut usage = Usage {
				input: count("promptTokenCount") - count("cachedContentTokenCount"),
				output: count("candidatesTokenCount") + count("thoughtsTokenCount"),
				cache_read: count("cachedContentTokenCount"),
				cache_write: 0.0,
				reasoning: Some(count("thoughtsTokenCount")),
				total_tokens: count("totalTokenCount"),
				..Usage::default()
			};
			calculate_cost(model, &mut usage);
			self.output.borrow_mut().usage = usage;
		}
		Ok(())
	}

	fn apply_text(&mut self, part: &Value, text: &str) {
		let is_thinking = is_thinking_part(part);
		let kind = if is_thinking {
			CurrentBlock::Thinking
		} else {
			CurrentBlock::Text
		};
		if self.current_block != Some(kind) {
			self.end_current_block();
			let block = match kind {
				CurrentBlock::Thinking => AssistantContent::Thinking(ThinkingContent::default()),
				CurrentBlock::Text => AssistantContent::Text(TextContent::default()),
			};
			self.output.borrow_mut().content.push(block);
			self.current_block = Some(kind);
			let content_index = self.block_index();
			let partial = self.output.clone();
			self.stream.push(match kind {
				CurrentBlock::Thinking => AssistantMessageEvent::ThinkingStart { content_index, partial },
				CurrentBlock::Text => AssistantMessageEvent::TextStart { content_index, partial },
			});
		}
		let thought_signature = part.get("thoughtSignature").and_then(Value::as_str);
		let content_index = self.block_index();
		match self.output.borrow_mut().content.last_mut() {
			Some(AssistantContent::Thinking(block)) => {
				block.thinking.push_str(text);
				block.thinking_signature = retain_thought_signature(block.thinking_signature.take(), thought_signature);
			}
			Some(AssistantContent::Text(block)) => {
				block.text.push_str(text);
				block.text_signature = retain_thought_signature(block.text_signature.take(), thought_signature);
			}
			_ => unreachable!("the current block is the last content block"),
		}
		let delta = text.to_string();
		let partial = self.output.clone();
		self.stream.push(match kind {
			CurrentBlock::Thinking => AssistantMessageEvent::ThinkingDelta {
				content_index,
				delta,
				partial,
			},
			CurrentBlock::Text => AssistantMessageEvent::TextDelta {
				content_index,
				delta,
				partial,
			},
		});
	}

	fn apply_function_call(&mut self, part: &Value, function_call: &Map<String, Value>) {
		self.end_current_block();

		// Generate unique ID if not provided or if it's a duplicate
		let provided_id = function_call
			.get("id")
			.and_then(Value::as_str)
			.filter(|id| !id.is_empty());
		let needs_new_id = provided_id.is_none_or(|provided_id| {
			self.output
				.borrow()
				.content
				.iter()
				.any(|block| matches!(block, AssistantContent::ToolCall(tool_call) if tool_call.id == provided_id))
		});
		let name = function_call.get("name");
		let tool_call_id = match provided_id {
			Some(provided_id) if !needs_new_id => provided_id.to_string(),
			_ => format!(
				"{}_{}_{}",
				name.map_or_else(|| "undefined".to_string(), js_string),
				number_to_string(pi_host::now_ms()),
				self.tool_call_counter.fetch_add(1, Ordering::Relaxed) + 1
			),
		};

		let tool_call = ToolCall {
			id: tool_call_id,
			name: name.and_then(Value::as_str).unwrap_or_default().to_string(),
			arguments: function_call
				.get("args")
				.and_then(Value::as_object)
				.cloned()
				.unwrap_or_default(),
			thought_signature: part
				.get("thoughtSignature")
				.and_then(Value::as_str)
				.filter(|signature| !signature.is_empty())
				.map(str::to_string),
			..ToolCall::default()
		};

		self.output
			.borrow_mut()
			.content
			.push(AssistantContent::ToolCall(tool_call.clone()));
		let content_index = self.block_index();
		self.stream.push(AssistantMessageEvent::ToolcallStart {
			content_index,
			partial: self.output.clone(),
		});
		self.stream.push(AssistantMessageEvent::ToolcallDelta {
			content_index,
			delta: json_stringify(&Value::Object(tool_call.arguments.clone())),
			partial: self.output.clone(),
		});
		self.stream.push(AssistantMessageEvent::ToolcallEnd {
			content_index,
			tool_call,
			partial: self.output.clone(),
		});
	}
}

/// `streamSimple`: `stream` with the provider-neutral simple options.
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

	let base = GoogleOptions {
		base: build_base_options(model, context, options.as_ref(), Some(&api_key)),
		tool_choice: options
			.as_ref()
			.and_then(|options| options.tool_choice)
			.map(|tool_choice| tool_choice_name(tool_choice).to_string()),
		thinking: None,
	};
	let with_thinking = |thinking: GoogleThinkingOptions| GoogleOptions {
		thinking: Some(thinking),
		..base.clone()
	};
	let disabled = GoogleThinkingOptions::default();
	let Some(reasoning) = options.as_ref().and_then(|options| options.reasoning) else {
		return Ok(stream(model, context, Some(with_thinking(disabled))));
	};

	let clamped_reasoning = clamp_thinking_level(model, ModelThinkingLevel::from(reasoning));
	if clamped_reasoning == ModelThinkingLevel::Off {
		return Ok(stream(model, context, Some(with_thinking(disabled))));
	}
	let resolved_level = resolve_google_thinking_level(model, clamped_reasoning)?;

	if uses_google_thinking_level(model) {
		return Ok(stream(
			model,
			context,
			Some(with_thinking(GoogleThinkingOptions {
				enabled: true,
				budget_tokens: None,
				level: Some(to_google_thinking_level(resolved_level)),
			})),
		));
	}

	Ok(stream(
		model,
		context,
		Some(with_thinking(GoogleThinkingOptions {
			enabled: true,
			budget_tokens: Some(get_google_budget(
				model,
				resolved_level,
				options.as_ref().and_then(|options| options.thinking_budgets.as_ref()),
			)),
			level: None,
		})),
	))
}

pub(crate) fn tool_choice_name(tool_choice: ToolChoice) -> &'static str {
	match tool_choice {
		ToolChoice::Auto => "auto",
		ToolChoice::None => "none",
	}
}

/// The google-generative-ai API module as `ProviderStreams`.
pub struct GoogleGenerativeAIStreams;

impl ProviderStreams for GoogleGenerativeAIStreams {
	fn stream(
		&self,
		model: &Model,
		context: &TranscriptContext,
		options: Option<ProviderStreamOptions>,
	) -> PiResult<AssistantMessageEventStream> {
		Ok(stream(model, context, options.map(GoogleOptions::from)))
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

/// `ProviderHeaders` of a plain header record.
pub(crate) fn to_provider_headers(headers: &IndexMap<String, String>) -> ProviderHeaders {
	headers
		.iter()
		.map(|(name, value)| (name.clone(), Some(value.clone())))
		.collect()
}

fn create_client(
	model: &Model,
	api_key: Option<&str>,
	options_headers: Option<&ProviderHeaders>,
	identity: Option<&ClientIdentity>,
) -> PiResult<GoogleGenAI> {
	let mut http_options = HttpOptions::default();
	if !model.base_url.is_empty() {
		http_options.base_url = Some(model.base_url.clone());
		http_options.api_version = Some(String::new()); // baseUrl already includes version path, don't append
	}
	let identity_headers = to_provider_headers(&get_identity_headers(
		ClientIdentityTarget {
			provider: &model.provider,
			base_url: &model.base_url,
		},
		identity,
	)?);
	let model_headers = model.headers.as_ref().map(to_provider_headers);
	if let Some(headers) =
		provider_headers_to_record(&[Some(&identity_headers), model_headers.as_ref(), options_headers])
	{
		http_options.headers = Some(headers);
	}

	GoogleGenAI::new(GoogleGenAIOptions {
		api_key: api_key.map(str::to_string),
		http_options: (http_options != HttpOptions::default()).then_some(http_options),
		..GoogleGenAIOptions::default()
	})
}

/// `buildParams`. Shared with `google_vertex`, whose options add only the client settings.
pub(crate) fn build_params(model: &Model, context: &TranscriptContext, options: &GoogleOptions) -> PiResult<Value> {
	let contents = convert_messages(model, context);
	let initial_system_message = get_initial_system_message(&context.messages);
	let current_tools = get_current_tools(&context.messages);

	let mut config = Map::new();
	if let Some(temperature) = options.temperature {
		config.insert("temperature".into(), json!(temperature));
	}
	if let Some(max_tokens) = options.max_tokens {
		config.insert("maxOutputTokens".into(), json!(max_tokens));
	}

	let supports_strict_mode = supports_google_strict_tool_sampling(&model.id);
	let function_calling_mode = if current_tools.is_empty() {
		None
	} else {
		resolve_google_function_calling_mode(&current_tools, options.tool_choice.as_deref(), supports_strict_mode)?
	};
	let system_instruction = initial_system_message.map(get_system_message_text).unwrap_or_default();
	if !system_instruction.is_empty() {
		config.insert(
			"systemInstruction".into(),
			Value::String(sanitize_surrogates(&system_instruction).to_string()),
		);
	}
	if !current_tools.is_empty()
		&& let Some(tools) = convert_tools(&current_tools, Some(false), Some(supports_strict_mode))?
	{
		config.insert("tools".into(), Value::Array(tools));
	}
	if let Some(mode) = function_calling_mode {
		config.insert(
			"toolConfig".into(),
			json!({ "functionCallingConfig": { "mode": mode.as_str() } }),
		);
	}

	match options.thinking {
		Some(thinking) if thinking.enabled && model.reasoning => {
			let mut thinking_config = Map::from_iter([("includeThoughts".to_string(), Value::Bool(true))]);
			if let Some(level) = thinking.level {
				thinking_config.insert(
					"thinkingLevel".into(),
					Value::String(to_google_sdk_thinking_level(level).as_str().into()),
				);
			} else if let Some(budget_tokens) = thinking.budget_tokens {
				thinking_config.insert("thinkingBudget".into(), json!(budget_tokens));
			}
			config.insert("thinkingConfig".into(), Value::Object(thinking_config));
		}
		Some(thinking) if model.reasoning && !thinking.enabled => {
			config.insert(
				"thinkingConfig".into(),
				Value::Object(get_disabled_google_thinking_config(model)?),
			);
		}
		_ => {}
	}

	if options.signal.as_ref().is_some_and(|signal| signal.aborted()) {
		return Err(PiError::new("Request aborted"));
	}

	Ok(json!({
		"model": model.id,
		"contents": contents,
		"config": config,
	}))
}

fn get_google_budget(
	model: &Model,
	level: ResolvedGoogleThinkingLevel,
	custom_budgets: Option<&ThinkingBudgets>,
) -> f64 {
	let custom_budget = custom_budgets.and_then(|budgets| match level {
		ResolvedGoogleThinkingLevel::Minimal => budgets.minimal,
		ResolvedGoogleThinkingLevel::Low => budgets.low,
		ResolvedGoogleThinkingLevel::Medium => budgets.medium,
		ResolvedGoogleThinkingLevel::High => budgets.high,
	});
	if let Some(custom_budget) = custom_budget {
		return custom_budget;
	}

	let budgets: [f64; 4] = if model.id.contains("2.5-pro") {
		[128.0, 2048.0, 8192.0, 32768.0]
	} else if model.id.contains("2.5-flash-lite") {
		[512.0, 2048.0, 8192.0, 24576.0]
	} else if model.id.contains("2.5-flash") {
		[128.0, 2048.0, 8192.0, 24576.0]
	} else {
		return -1.0;
	};
	match level {
		ResolvedGoogleThinkingLevel::Minimal => budgets[0],
		ResolvedGoogleThinkingLevel::Low => budgets[1],
		ResolvedGoogleThinkingLevel::Medium => budgets[2],
		ResolvedGoogleThinkingLevel::High => budgets[3],
	}
}
