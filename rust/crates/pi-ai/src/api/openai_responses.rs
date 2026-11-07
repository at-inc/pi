//! Port of `packages/ai/src/api/openai-responses.ts`.
//! PORT-STATUS: ported
//!
//! - `OpenAIResponsesOptions` reaches `ProviderStreams::stream` as `ProviderStreamOptions.extra` keys:
//!   `reasoningEffort` (a thinking level string), `reasoningSummary` (string or `null`), `serviceTier` and
//!   `toolChoice` (any JSON). An unknown `reasoningEffort` string reads as absent, and a `reasoningSummary` that is
//!   not a string reads as `null` (the TS sends a truthy non-string, such as `true`, as the summary).
//! - `getCompat` reads `model.compat` through `openai_responses_compat()`: a field of the wrong type reads as absent
//!   and gets its default, where the TS uses the value as it is.
//! - The async body runs on the host executor from the first step, so a setup failure (a missing API key) is pushed
//!   when the task runs, not before `stream()` returns; the output timestamp is still taken in `stream()`.
//! - `onPayload` receives a copy of the params, so a hook that mutates them in place and returns `undefined` loses
//!   the mutation; the TS sends it.

use std::rc::Rc;

use indexmap::IndexMap;
use pi_host::js::math_max;
use pi_host::{PiError, PiResult};
use serde_json::{Map, Value, json};

use crate::api::constrained_sampling::create_grammar_tool_input_properties;
use crate::api::github_copilot_headers::{build_copilot_dynamic_headers, has_copilot_vision_input};
use crate::api::openai_client::{ApiData, ClientOptions, OpenAI, RequestOptions};
use crate::api::openai_prompt_cache::clamp_openai_prompt_cache_key;
use crate::api::openai_responses_shared::{
	ConvertResponsesMessagesOptions, ConvertResponsesToolsOptions, OpenAIResponsesStreamOptions,
	convert_responses_messages, convert_responses_tools, get_responses_tool_support, process_responses_stream,
};
use crate::api::simple_options::{build_base_options, resolve_sampling_params};
use crate::models::clamp_thinking_level;
use crate::types::{
	AssistantMessage, AssistantMessageEvent, CacheRetention, Model, ModelThinkingLevel, ProviderEnv, ProviderHeaders,
	ProviderResponse, ProviderStreamOptions, ProviderStreams, SessionAffinityFormat, SharedAssistantMessage,
	SimpleStreamOptions, StopReason, StreamOptions, ThinkingLevel, ToolChoice, TranscriptContext, Usage, UsageCost,
};
use crate::utils::diagnostics::{append_assistant_message_diagnostic, create_provider_error_diagnostic};
use crate::utils::error_body::{format_provider_error, normalize_provider_error};
use crate::utils::event_stream::AssistantMessageEventStream;
use crate::utils::headers::headers_to_record;
use crate::utils::identity::{ClientIdentity, ClientIdentityTarget, get_identity_headers};
use crate::utils::provider_env::get_provider_env_value;
use crate::utils::provider_retry::{ProviderRetryOptions, retry_provider_request};
use crate::utils::text::js_trim;
use crate::utils::transcript::{get_declared_tools, resolve_transcript, resolve_transcript_tools};

const OPENAI_TOOL_CALL_PROVIDERS: [&str; 3] = ["openai", "openai-codex", "opencode"];
// OpenAI Responses rejects max_output_tokens below 16: https://github.com/earendil-works/pi/issues/6265
const OPENAI_RESPONSES_MIN_OUTPUT_TOKENS: f64 = 16.0;
const CHATGPT_USAGE_URL: &str = "https://chatgpt.com/settings/usage";
const CHATGPT_WORKSPACE_LIMIT_HINT: &str = "You've reached your ChatGPT workspace's usage limit. Ask your workspace admin to increase it at admin.openai.com → Workspace → Usage limits.";
// Fixed reasoning.effort while configuration_update items carry the thinking level.
const CONFIGURATION_UPDATE_REQUEST_EFFORT: &str = "medium";

/// OpenAI API keys start with `sk-`; a different credential sent directly to OpenAI
/// is a Sign in with ChatGPT access token.
fn is_chatgpt_sign_in(model: &Model, api_key: Option<&str>) -> bool {
	model.provider == "openai"
		&& model.base_url == "https://api.openai.com/v1"
		&& api_key.is_some_and(|api_key| !api_key.starts_with("sk-"))
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

fn get_client_api_key(provider: &str, api_key: Option<&str>, headers: Option<&ProviderHeaders>) -> PiResult<String> {
	if let Some(api_key) = api_key.filter(|api_key| !api_key.is_empty()) {
		return Ok(api_key.to_string());
	}
	if has_header(headers, "authorization") || has_header(headers, "cf-aig-authorization") {
		return Ok("unused".into());
	}
	Err(PiError::new(format!("No API key for provider: {provider}")))
}

fn detect_session_affinity_format(model: &Model) -> SessionAffinityFormat {
	if model.provider == "openrouter" || model.base_url.contains("openrouter.ai") {
		SessionAffinityFormat::Openrouter
	} else {
		SessionAffinityFormat::Openai
	}
}

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

/// `Required<OpenAIResponsesCompat>`.
#[derive(Clone, Copy, Debug)]
struct ResolvedCompat {
	supports_mid_convo_system_messages: bool,
	session_affinity_format: SessionAffinityFormat,
	supports_long_cache_retention: bool,
	supports_strict_mode: bool,
	supports_openai_grammar_tools: bool,
	supports_additional_tools: bool,
	supports_tool_search: bool,
	supports_explicit_prompt_cache_mode: bool,
	supports_max_output_tokens: bool,
	supports_mid_convo_effort: bool,
}

fn get_compat(model: &Model) -> ResolvedCompat {
	let compat = model.openai_responses_compat();
	ResolvedCompat {
		supports_mid_convo_system_messages: compat.supports_mid_convo_system_messages.unwrap_or(false),
		session_affinity_format: compat
			.session_affinity_format
			.unwrap_or_else(|| detect_session_affinity_format(model)),
		supports_long_cache_retention: compat.supports_long_cache_retention.unwrap_or(true),
		supports_strict_mode: compat.supports_strict_mode.unwrap_or(false),
		supports_openai_grammar_tools: compat.supports_openai_grammar_tools.unwrap_or(false),
		supports_additional_tools: compat.supports_additional_tools.unwrap_or(false),
		supports_tool_search: compat.supports_tool_search.unwrap_or(false),
		supports_explicit_prompt_cache_mode: compat.supports_explicit_prompt_cache_mode.unwrap_or(false),
		supports_max_output_tokens: compat.supports_max_output_tokens.unwrap_or(true),
		supports_mid_convo_effort: compat.supports_mid_convo_effort.unwrap_or(false),
	}
}

/// `model.thinkingLevelMap?.[effort] ?? effort`.
fn mapped_effort(model: &Model, effort: ThinkingLevel) -> String {
	model
		.thinking_level_map
		.as_ref()
		.and_then(|map| map.get(effort.into()))
		.flatten()
		.unwrap_or(effort.as_str())
		.to_string()
}

/// Effort to send in configuration_update items, or undefined to send it as reasoning.effort.
fn get_configuration_update_effort(
	model: &Model,
	options: Option<&OpenAIResponsesOptions>,
	compat: &ResolvedCompat,
) -> Option<String> {
	let options = options?;
	let effort = options.reasoning_effort?;
	if !compat.supports_mid_convo_effort
		// Sign in with ChatGPT rejects configuration_update items.
		|| is_chatgpt_sign_in(model, options.api_key.as_deref())
	{
		return None;
	}
	Some(mapped_effort(model, effort))
}

fn get_prompt_cache_retention(compat: &ResolvedCompat, cache_retention: CacheRetention) -> Option<&'static str> {
	(cache_retention == CacheRetention::Long
		&& compat.supports_long_cache_retention
		&& !compat.supports_explicit_prompt_cache_mode)
		.then_some("24h")
}

fn get_prompt_cache_options(compat: &ResolvedCompat, cache_retention: CacheRetention) -> Option<Value> {
	if !compat.supports_explicit_prompt_cache_mode {
		return None;
	}
	if cache_retention == CacheRetention::None {
		return Some(json!({ "mode": "explicit" }));
	}
	if cache_retention == CacheRetention::Long && compat.supports_long_cache_retention {
		return Some(json!({ "ttl": "30m" }));
	}
	None
}

/// OpenAI Responses-specific options.
#[derive(Clone, Default)]
pub struct OpenAIResponsesOptions {
	pub base: StreamOptions,
	pub reasoning_effort: Option<ThinkingLevel>,
	/// `"auto" | "detailed" | "concise" | null`: `Some(None)` is `null`.
	pub reasoning_summary: Option<Option<String>>,
	/// `ResponseCreateParamsStreaming["service_tier"]`, sent as given.
	pub service_tier: Option<Value>,
	/// `ResponseCreateParamsStreaming["tool_choice"]`, sent as given.
	pub tool_choice: Option<Value>,
}

impl std::ops::Deref for OpenAIResponsesOptions {
	type Target = StreamOptions;

	fn deref(&self) -> &Self::Target {
		&self.base
	}
}

impl std::ops::DerefMut for OpenAIResponsesOptions {
	fn deref_mut(&mut self) -> &mut Self::Target {
		&mut self.base
	}
}

impl OpenAIResponsesOptions {
	/// Reads the option keys of this API from `ProviderStreamOptions.extra`.
	pub fn from_provider_options(options: ProviderStreamOptions) -> Self {
		let ProviderStreamOptions { base, extra } = options;
		Self {
			base,
			reasoning_effort: extra
				.get("reasoningEffort")
				.and_then(|value| serde_json::from_value(value.clone()).ok()),
			reasoning_summary: match extra.get("reasoningSummary") {
				None => None,
				Some(Value::String(summary)) => Some(Some(summary.clone())),
				Some(_) => Some(None),
			},
			service_tier: extra.get("serviceTier").cloned(),
			tool_choice: extra.get("toolChoice").cloned(),
		}
	}
}

/// The `openai-responses.ts` module as `ProviderStreams`.
pub struct OpenAIResponsesApi;

impl ProviderStreams for OpenAIResponsesApi {
	fn stream(
		&self,
		model: &Model,
		context: &TranscriptContext,
		options: Option<ProviderStreamOptions>,
	) -> PiResult<AssistantMessageEventStream> {
		Ok(stream(
			model,
			context,
			options.map(OpenAIResponsesOptions::from_provider_options),
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

/// Generate function for OpenAI Responses API
pub fn stream(
	model: &Model,
	context: &TranscriptContext,
	options: Option<OpenAIResponsesOptions>,
) -> AssistantMessageEventStream {
	let stream = AssistantMessageEventStream::new();
	let normalized_context = resolve_transcript(context, Some(get_compat(model).supports_mid_convo_system_messages));

	let compat = get_compat(model);
	let configuration_update_effort = get_configuration_update_effort(model, options.as_ref(), &compat);
	let output: SharedAssistantMessage = Rc::new(std::cell::RefCell::new(AssistantMessage {
		content: Vec::new(),
		api: model.api.clone(),
		provider: model.provider.clone(),
		model: model.id.clone(),
		provider_thinking_level: configuration_update_effort.clone(),
		usage: Usage {
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
		let result = run_stream(
			&model,
			&normalized_context,
			options,
			&compat,
			configuration_update_effort.as_deref(),
			&output,
			&target,
		)
		.await;
		let Err(error) = result else {
			return;
		};
		let aborted = options
			.and_then(|options| options.signal.as_ref())
			.is_some_and(|signal| signal.aborted());
		let stop_reason = if aborted {
			StopReason::Aborted
		} else {
			StopReason::Error
		};
		{
			let mut output = output.borrow_mut();
			output.stop_reason = stop_reason;
			let provider = if model.provider == "openai" {
				"OpenAI"
			} else {
				&model.provider
			};
			let error_message = format_provider_error(
				&normalize_provider_error(&error),
				Some(&format!("{provider} API error")),
			);
			output.error_message = Some(if error_message.contains("subscription_sharing_usage_limit_exceeded") {
				format!("{error_message}\nCheck your ChatGPT usage: {CHATGPT_USAGE_URL}")
			} else if error_message.contains("subscription_sharing_workplace_individual_usage_limit_exceeded") {
				format!("{error_message}\n{CHATGPT_WORKSPACE_LIMIT_HINT}")
			} else {
				error_message
			});
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

async fn run_stream(
	model: &Model,
	normalized_context: &TranscriptContext,
	options: Option<&OpenAIResponsesOptions>,
	compat: &ResolvedCompat,
	configuration_update_effort: Option<&str>,
	output: &SharedAssistantMessage,
	stream: &AssistantMessageEventStream,
) -> PiResult<()> {
	// Create OpenAI client
	let api_key = get_client_api_key(
		&model.provider,
		options.and_then(|options| options.api_key.as_deref()),
		options.and_then(|options| options.headers.as_ref()),
	)?;
	let cache_retention = resolve_cache_retention(
		options.and_then(|options| options.cache_retention),
		options.and_then(|options| options.env.as_ref()),
	);
	let cache_session_id = if cache_retention == CacheRetention::None {
		None
	} else {
		options.and_then(|options| options.session_id.as_deref())
	};
	let grammar_tool_input_properties = create_grammar_tool_input_properties(
		Some(&get_declared_tools(&normalized_context.messages)),
		compat.supports_openai_grammar_tools,
	)?;
	let client = create_client(
		model,
		normalized_context,
		&api_key,
		options.and_then(|options| options.headers.as_ref()),
		options.and_then(|options| options.identity.as_ref()),
		options.and_then(|options| options.fetch.clone()),
		cache_session_id,
	)?;
	let mut params = build_params(
		model,
		normalized_context,
		options,
		compat,
		&grammar_tool_input_properties,
		configuration_update_effort,
	)?;
	if let Some(on_payload) = options.and_then(|options| options.on_payload.as_ref())
		&& let Some(next_params) = on_payload(params.clone(), model).await?
	{
		params = next_params;
	}
	let request_options = RequestOptions {
		signal: options.and_then(|options| options.signal.clone()),
		timeout: options.and_then(|options| options.timeout_ms),
		max_retries: Some(0.0),
		headers: None,
	};
	let with_response = retry_provider_request(
		|| client.post_json("/responses", &params, &request_options),
		ProviderRetryOptions {
			max_retries: options.and_then(|options| options.max_retries),
			max_retry_delay_ms: options.and_then(|options| options.max_retry_delay_ms),
			signal: options.and_then(|options| options.signal.clone()),
			no_retry_statuses: None,
		},
	)
	.await?;
	if let Some(on_response) = options.and_then(|options| options.on_response.as_ref()) {
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

	let ApiData::Stream(openai_stream) = with_response.data else {
		return Err(PiError::type_error("openaiStream is not async iterable"));
	};
	let model_id = model.id.clone();
	process_responses_stream(
		openai_stream,
		output,
		stream,
		model,
		Some(&OpenAIResponsesStreamOptions {
			on_provider_stream_event: options.and_then(|options| options.on_provider_stream_event.clone()),
			service_tier: options
				.and_then(|options| options.service_tier.as_ref())
				.and_then(Value::as_str)
				.map(str::to_string),
			grammar_tool_input_properties: Some(grammar_tool_input_properties),
			apply_service_tier_pricing: Some(Rc::new(move |usage: &mut Usage, service_tier: Option<&str>| {
				apply_service_tier_pricing(usage, service_tier, &model_id)
			})),
			..OpenAIResponsesStreamOptions::default()
		}),
	)
	.await?;

	if options
		.and_then(|options| options.signal.as_ref())
		.is_some_and(|signal| signal.aborted())
	{
		return Err(PiError::new("Request was aborted"));
	}

	let (stop_reason, error_message) = {
		let output = output.borrow();
		(output.stop_reason, output.error_message.clone())
	};
	if stop_reason == StopReason::Pending {
		return Err(PiError::new("OpenAI Responses stream ended without a stop reason"));
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

	let base = build_base_options(
		model,
		context,
		options.as_ref(),
		options.as_ref().and_then(|options| options.api_key.as_deref()),
	);
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
	let reasoning_effort = clamped_reasoning.and_then(thinking_level_of);

	Ok(stream(
		model,
		context,
		Some(OpenAIResponsesOptions {
			base,
			tool_choice,
			reasoning_effort,
			..OpenAIResponsesOptions::default()
		}),
	))
}

/// A model thinking level other than `off`.
pub(crate) fn thinking_level_of(level: ModelThinkingLevel) -> Option<ThinkingLevel> {
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

fn create_client(
	model: &Model,
	context: &TranscriptContext,
	api_key: &str,
	options_headers: Option<&ProviderHeaders>,
	identity: Option<&ClientIdentity>,
	fetch: Option<pi_host::FetchFunction>,
	session_id: Option<&str>,
) -> PiResult<OpenAI> {
	let compat = get_compat(model);
	let mut headers: ProviderHeaders = IndexMap::new();
	for (name, value) in get_identity_headers(
		ClientIdentityTarget {
			provider: &model.provider,
			base_url: &model.base_url,
		},
		identity,
	)? {
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

	if let Some(session_id) = session_id.filter(|session_id| !session_id.is_empty()) {
		if compat.session_affinity_format == SessionAffinityFormat::Openrouter {
			headers.insert("x-session-id".into(), Some(session_id.to_string()));
		} else {
			if compat.session_affinity_format == SessionAffinityFormat::Openai {
				headers.insert("session_id".into(), Some(session_id.to_string()));
			}
			headers.insert("x-client-request-id".into(), Some(session_id.to_string()));
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

fn build_params(
	model: &Model,
	context: &TranscriptContext,
	options: Option<&OpenAIResponsesOptions>,
	compat: &ResolvedCompat,
	grammar_tool_input_properties: &IndexMap<String, String>,
	configuration_update_effort: Option<&str>,
) -> PiResult<Value> {
	let transcript_tools = resolve_transcript_tools(
		&context.messages,
		get_responses_tool_support(
			Some(compat.supports_mid_convo_system_messages),
			Some(compat.supports_additional_tools),
			Some(compat.supports_tool_search),
		),
	);
	let messages = convert_responses_messages(
		model,
		context,
		&OPENAI_TOOL_CALL_PROVIDERS,
		Some(&ConvertResponsesMessagesOptions {
			grammar_tool_input_properties: Some(grammar_tool_input_properties.clone()),
			supports_mid_convo_system_messages: Some(compat.supports_mid_convo_system_messages),
			supports_additional_tools: Some(compat.supports_additional_tools),
			supports_tool_search: Some(compat.supports_tool_search),
			tool_options: Some(ConvertResponsesToolsOptions {
				supports_strict_mode: Some(compat.supports_strict_mode),
				supports_openai_grammar_tools: Some(compat.supports_openai_grammar_tools),
				..ConvertResponsesToolsOptions::default()
			}),
			configuration_update_effort: configuration_update_effort.map(str::to_string),
			..ConvertResponsesMessagesOptions::default()
		}),
	)?;

	let cache_retention = resolve_cache_retention(
		options.and_then(|options| options.cache_retention),
		options.and_then(|options| options.env.as_ref()),
	);
	// Sign in with ChatGPT rejects these request fields.
	let omit_unsupported_fields = is_chatgpt_sign_in(model, options.and_then(|options| options.api_key.as_deref()));
	let mut params = Map::new();
	params.insert("model".into(), json!(model.id));
	params.insert("input".into(), Value::Array(messages));
	params.insert("stream".into(), json!(true));
	if cache_retention != CacheRetention::None
		&& let Some(key) = clamp_openai_prompt_cache_key(options.and_then(|options| options.session_id.as_deref()))
	{
		params.insert("prompt_cache_key".into(), json!(key));
	}
	if !omit_unsupported_fields && let Some(retention) = get_prompt_cache_retention(compat, cache_retention) {
		params.insert("prompt_cache_retention".into(), json!(retention));
	}
	if !omit_unsupported_fields && let Some(cache_options) = get_prompt_cache_options(compat, cache_retention) {
		params.insert("prompt_cache_options".into(), cache_options);
	}
	params.insert("store".into(), json!(false));

	if let Some(max_tokens) = options
		.and_then(|options| options.max_tokens)
		.filter(|max_tokens| *max_tokens != 0.0 && !max_tokens.is_nan())
		&& compat.supports_max_output_tokens
		&& !omit_unsupported_fields
	{
		params.insert(
			"max_output_tokens".into(),
			json!(math_max(max_tokens, OPENAI_RESPONSES_MIN_OUTPUT_TOKENS)),
		);
	}

	if let Some(temperature) = options.and_then(|options| options.temperature)
		&& !omit_unsupported_fields
	{
		params.insert("temperature".into(), json!(temperature));
	}

	if let Some(service_tier) = options.and_then(|options| options.service_tier.as_ref()) {
		params.insert("service_tier".into(), service_tier.clone());
	}

	if !transcript_tools.request_tools.is_empty() {
		params.insert(
			"tools".into(),
			Value::Array(convert_responses_tools(
				&transcript_tools.request_tools,
				Some(&ConvertResponsesToolsOptions {
					supports_strict_mode: Some(compat.supports_strict_mode),
					supports_openai_grammar_tools: Some(compat.supports_openai_grammar_tools),
					..ConvertResponsesToolsOptions::default()
				}),
			)?),
		);
	}

	if let Some(tool_choice) = options.and_then(|options| options.tool_choice.as_ref()) {
		params.insert("tool_choice".into(), tool_choice.clone());
	}

	let reasoning_summary = options
		.and_then(|options| options.reasoning_summary.as_ref())
		.and_then(Option::as_deref)
		.filter(|summary| !summary.is_empty());
	let reasoning_effort: Option<ModelThinkingLevel> = match options.and_then(|options| options.reasoning_effort) {
		Some(effort) => Some(effort.into()),
		None => reasoning_summary.map(|_| ModelThinkingLevel::Medium),
	};
	if model.reasoning {
		if let Some(reasoning_effort) = reasoning_effort {
			let requested_effort = match options.and_then(|options| options.reasoning_effort) {
				Some(effort) => mapped_effort(model, effort),
				None => reasoning_effort.as_str().to_string(),
			};
			let effort = match configuration_update_effort {
				None => requested_effort,
				Some(_) => CONFIGURATION_UPDATE_REQUEST_EFFORT.to_string(),
			};
			params.insert(
				"reasoning".into(),
				json!({ "effort": effort, "summary": reasoning_summary.unwrap_or("auto") }),
			);
			params.insert("include".into(), json!(["reasoning.encrypted_content"]));
		} else {
			let off = model
				.thinking_level_map
				.as_ref()
				.and_then(|map| map.get(ModelThinkingLevel::Off));
			if model.provider != "github-copilot" && off != Some(None) {
				params.insert("reasoning".into(), json!({ "effort": off.flatten().unwrap_or("none") }));
			}
		}
		if model.provider == "xai" {
			params.insert("include".into(), json!(["reasoning.encrypted_content"]));
		}
	}

	// Last so model and request sampling parameters override named request fields.
	let sampling_params = resolve_sampling_params(
		model,
		reasoning_effort.unwrap_or(ModelThinkingLevel::Off),
		options.and_then(|options| options.sampling_params.as_ref()),
	);
	if let Some(sampling_params) = sampling_params {
		for (key, value) in sampling_params {
			params.insert(key, value);
		}
	}

	Ok(Value::Object(params))
}

fn get_service_tier_cost_multiplier(model_id: &str, service_tier: Option<&str>) -> f64 {
	match service_tier {
		Some("flex") => 0.5,
		Some("priority" | "fast") => {
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
