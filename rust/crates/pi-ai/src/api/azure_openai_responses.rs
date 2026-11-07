//! Port of `packages/ai/src/api/azure-openai-responses.ts`.
//! PORT-STATUS: ported
//!
//! - `AzureOpenAIResponsesOptions` reaches `ProviderStreams::stream` as `ProviderStreamOptions.extra` keys:
//!   `reasoningEffort`, `reasoningSummary` and `toolChoice` as in `openai_responses.rs`, and the `azure*` string keys
//!   of `AzureEndpointOptions`. A `reasoningSummary` that is not a string reads as `null`, as there.
//! - `model.compat` is read through `openai_responses_compat()`: a field of the wrong type reads as absent.
//! - The async body runs on the host executor from the first step; the output timestamp is taken in `stream()`.
//! - `onPayload` receives a copy of the params, so a hook that mutates them in place and returns `undefined` loses
//!   the mutation; the TS sends it.
//! - `new AzureOpenAI(...)` is `new_azure_openai`, built on the `OpenAI` client of `openai_client.rs`: it adds the
//!   `api-version` query, sends `api-key` in place of `Authorization`, and names the client `AzureOpenAI` in
//!   `User-Agent`, as default headers. `OPENAI_CUSTOM_HEADERS` therefore cannot override `User-Agent` or `api-key`
//!   (the SDK applies it after them). Requests that carry `api-key` go out with `redirect: "manual"`, as in the SDK.

use std::rc::Rc;

use indexmap::IndexMap;
use pi_host::js::math_max;
use pi_host::{PiError, PiResult};
use serde_json::{Map, Value, json};

use crate::api::azure_openai_config::{AzureEndpointOptions, resolve_azure_config, resolve_deployment_name};
use crate::api::constrained_sampling::create_grammar_tool_input_properties;
use crate::api::openai_client::{ApiData, ClientOptions, OpenAI, RequestOptions, VERSION};
use crate::api::openai_prompt_cache::clamp_openai_prompt_cache_key;
use crate::api::openai_responses::thinking_level_of;
use crate::api::openai_responses_shared::{
	ConvertResponsesMessagesOptions, ConvertResponsesToolsOptions, OpenAIResponsesStreamOptions,
	convert_responses_messages, convert_responses_tools, get_responses_tool_support, process_responses_stream,
};
use crate::api::simple_options::{build_base_options, resolve_sampling_params};
use crate::models::clamp_thinking_level;
use crate::types::{
	AssistantMessage, AssistantMessageEvent, Model, ModelThinkingLevel, ProviderHeaders, ProviderResponse,
	ProviderStreamOptions, ProviderStreams, SharedAssistantMessage, SimpleStreamOptions, StopReason, ThinkingLevel,
	ToolChoice, TranscriptContext, Usage, UsageCost, api,
};
use crate::utils::error_body::{format_provider_error, normalize_provider_error};
use crate::utils::event_stream::AssistantMessageEventStream;
use crate::utils::headers::headers_to_record;
use crate::utils::identity::{ClientIdentityTarget, get_identity_headers};
use crate::utils::provider_retry::{ProviderRetryOptions, retry_provider_request};
use crate::utils::transcript::{get_declared_tools, resolve_transcript, resolve_transcript_tools};

const AZURE_TOOL_CALL_PROVIDERS: [&str; 4] = ["openai", "openai-codex", "opencode", "azure"];
// OpenAI Responses rejects max_output_tokens below 16: https://github.com/earendil-works/pi/issues/6265
const OPENAI_RESPONSES_MIN_OUTPUT_TOKENS: f64 = 16.0;

fn format_azure_openai_error(error: &PiError) -> String {
	format_provider_error(&normalize_provider_error(error), Some("Azure OpenAI API error"))
}

/// Azure OpenAI Responses-specific options
#[derive(Clone, Default)]
pub struct AzureOpenAIResponsesOptions {
	pub base: AzureEndpointOptions,
	pub reasoning_effort: Option<ThinkingLevel>,
	/// `ResponseCreateParamsStreaming["tool_choice"]`, sent as given.
	pub tool_choice: Option<Value>,
	/// `"auto" | "detailed" | "concise" | null`: `Some(None)` is `null`.
	pub reasoning_summary: Option<Option<String>>,
}

impl std::ops::Deref for AzureOpenAIResponsesOptions {
	type Target = AzureEndpointOptions;

	fn deref(&self) -> &Self::Target {
		&self.base
	}
}

impl std::ops::DerefMut for AzureOpenAIResponsesOptions {
	fn deref_mut(&mut self) -> &mut Self::Target {
		&mut self.base
	}
}

impl AzureOpenAIResponsesOptions {
	/// Reads the option keys of this API from `ProviderStreamOptions.extra`.
	pub fn from_provider_options(options: ProviderStreamOptions) -> Self {
		let ProviderStreamOptions { base, extra } = options;
		let field = |name: &str| extra.get(name).and_then(Value::as_str).map(str::to_string);
		Self {
			base: AzureEndpointOptions {
				azure_api_version: field("azureApiVersion"),
				azure_resource_name: field("azureResourceName"),
				azure_base_url: field("azureBaseUrl"),
				azure_deployment_name: field("azureDeploymentName"),
				base,
			},
			reasoning_effort: extra
				.get("reasoningEffort")
				.and_then(|value| serde_json::from_value(value.clone()).ok()),
			tool_choice: extra.get("toolChoice").cloned(),
			reasoning_summary: match extra.get("reasoningSummary") {
				None => None,
				Some(Value::String(summary)) => Some(Some(summary.clone())),
				Some(_) => Some(None),
			},
		}
	}
}

/// The `azure-openai-responses.ts` module as `ProviderStreams`.
pub struct AzureOpenAIResponsesApi;

impl ProviderStreams for AzureOpenAIResponsesApi {
	fn stream(
		&self,
		model: &Model,
		context: &TranscriptContext,
		options: Option<ProviderStreamOptions>,
	) -> PiResult<AssistantMessageEventStream> {
		Ok(stream(
			model,
			context,
			options.map(AzureOpenAIResponsesOptions::from_provider_options),
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

/// Generate function for Azure OpenAI Responses API
pub fn stream(
	model: &Model,
	context: &TranscriptContext,
	options: Option<AzureOpenAIResponsesOptions>,
) -> AssistantMessageEventStream {
	let stream = AssistantMessageEventStream::new();
	let normalized_context = resolve_transcript(
		context,
		model.openai_responses_compat().supports_mid_convo_system_messages,
	);

	let deployment_name = resolve_deployment_name(model, options.as_ref().map(|options| &options.base));
	let output: SharedAssistantMessage = Rc::new(std::cell::RefCell::new(AssistantMessage {
		content: Vec::new(),
		api: api::AZURE_OPENAI_RESPONSES.into(),
		provider: model.provider.clone(),
		model: model.id.clone(),
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
		let result = run_stream(&model, &normalized_context, options, &deployment_name, &output, &target).await;
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
			output.error_message = Some(format_azure_openai_error(&error));
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
	options: Option<&AzureOpenAIResponsesOptions>,
	deployment_name: &str,
	output: &SharedAssistantMessage,
	stream: &AssistantMessageEventStream,
) -> PiResult<()> {
	// Create Azure OpenAI client
	let Some(api_key) = options
		.and_then(|options| options.api_key.as_deref())
		.filter(|api_key| !api_key.is_empty())
	else {
		return Err(PiError::new(format!("No API key for provider: {}", model.provider)));
	};
	let client = create_client(model, api_key, options)?;
	let grammar_tool_input_properties = create_grammar_tool_input_properties(
		Some(&get_declared_tools(&normalized_context.messages)),
		model
			.openai_responses_compat()
			.supports_openai_grammar_tools
			.unwrap_or(false),
	)?;
	let mut params = build_params(
		model,
		normalized_context,
		options,
		deployment_name,
		&grammar_tool_input_properties,
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
	process_responses_stream(
		openai_stream,
		output,
		stream,
		model,
		Some(&OpenAIResponsesStreamOptions {
			on_provider_stream_event: options.and_then(|options| options.on_provider_stream_event.clone()),
			grammar_tool_input_properties: Some(grammar_tool_input_properties),
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
		return Err(PiError::new(
			"Azure OpenAI Responses stream ended without a stop reason",
		));
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
	let Some(api_key) = options
		.as_ref()
		.and_then(|options| options.api_key.clone())
		.filter(|api_key| !api_key.is_empty())
	else {
		return Err(PiError::new(format!("No API key for provider: {}", model.provider)));
	};

	let base = build_base_options(model, context, options.as_ref(), Some(&api_key));
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
		Some(AzureOpenAIResponsesOptions {
			base: AzureEndpointOptions {
				base,
				..AzureEndpointOptions::default()
			},
			tool_choice,
			reasoning_effort,
			..AzureOpenAIResponsesOptions::default()
		}),
	))
}

fn create_client(model: &Model, api_key: &str, options: Option<&AzureOpenAIResponsesOptions>) -> PiResult<OpenAI> {
	let mut headers: ProviderHeaders = IndexMap::new();
	for (name, value) in get_identity_headers(
		ClientIdentityTarget {
			provider: &model.provider,
			base_url: &model.base_url,
		},
		options.and_then(|options| options.identity.as_ref()),
	)? {
		headers.insert(name, Some(value));
	}
	for (name, value) in model.headers.iter().flatten() {
		headers.insert(name.clone(), Some(value.clone()));
	}

	if let Some(options_headers) = options.and_then(|options| options.headers.as_ref()) {
		for (name, value) in options_headers {
			headers.insert(name.clone(), value.clone());
		}
	}

	let config = resolve_azure_config(model, options.map(|options| &options.base))?;

	new_azure_openai(
		api_key,
		&config.api_version,
		&config.base_url,
		headers,
		options.and_then(|options| options.fetch.clone()),
	)
}

/// `new AzureOpenAI({ apiKey, apiVersion, baseURL, defaultHeaders, fetch })`: the `AzureOpenAI` constructor checks and
/// `authHeaders`, on top of the `OpenAI` client.
fn new_azure_openai(
	api_key: &str,
	api_version: &str,
	base_url: &str,
	default_headers: ProviderHeaders,
	fetch: Option<pi_host::FetchFunction>,
) -> PiResult<OpenAI> {
	if api_version.is_empty() {
		return Err(PiError::new(
			"The OPENAI_API_VERSION environment variable is missing or empty; either provide it, or instantiate the AzureOpenAI client with an apiVersion option, like new AzureOpenAI({ apiVersion: 'My API Version' }).",
		));
	}
	if api_key.is_empty() {
		return Err(PiError::new(
			"Missing credentials. Please pass one of `apiKey` and `azureADTokenProvider`, or set the `AZURE_OPENAI_API_KEY` environment variable.",
		));
	}
	let base_url = if base_url.is_empty() {
		let Some(endpoint) = pi_host::env("AZURE_OPENAI_ENDPOINT").filter(|endpoint| !endpoint.is_empty()) else {
			return Err(PiError::new(
				"Must provide one of the `baseURL` or `endpoint` arguments, or the `AZURE_OPENAI_ENDPOINT` environment variable",
			));
		};
		format!("{}/openai", endpoint.trim_end_matches('/'))
	} else {
		base_url.to_string()
	};
	let mut headers: ProviderHeaders = IndexMap::from([
		("User-Agent".to_string(), Some(format!("AzureOpenAI/JS {VERSION}"))),
		("Authorization".to_string(), None),
		("api-key".to_string(), Some(api_key.to_string())),
	]);
	headers.extend(default_headers);
	OpenAI::new(ClientOptions {
		api_key: Some(api_key.to_string()),
		base_url: Some(base_url),
		fetch,
		default_headers: Some(headers),
		default_query: Some(IndexMap::from([("api-version".to_string(), api_version.to_string())])),
		manual_redirect_with_api_key: true,
		..ClientOptions::default()
	})
}

fn build_params(
	model: &Model,
	context: &TranscriptContext,
	options: Option<&AzureOpenAIResponsesOptions>,
	deployment_name: &str,
	grammar_tool_input_properties: &IndexMap<String, String>,
) -> PiResult<Value> {
	let compat = model.openai_responses_compat();
	let supports_additional_tools = compat.supports_additional_tools.unwrap_or(false);
	let supports_tool_search = compat.supports_tool_search.unwrap_or(false);
	let supports_mid_convo_system_messages = compat.supports_mid_convo_system_messages.unwrap_or(false);
	let tool_options = ConvertResponsesToolsOptions {
		supports_strict_mode: Some(compat.supports_strict_mode.unwrap_or(true)),
		supports_openai_grammar_tools: Some(compat.supports_openai_grammar_tools.unwrap_or(false)),
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
		&AZURE_TOOL_CALL_PROVIDERS,
		Some(&ConvertResponsesMessagesOptions {
			grammar_tool_input_properties: Some(grammar_tool_input_properties.clone()),
			supports_mid_convo_system_messages: Some(supports_mid_convo_system_messages),
			supports_additional_tools: Some(supports_additional_tools),
			supports_tool_search: Some(supports_tool_search),
			tool_options: Some(tool_options.clone()),
			..ConvertResponsesMessagesOptions::default()
		}),
	)?;

	let mut params = Map::new();
	params.insert("model".into(), json!(deployment_name));
	params.insert("input".into(), Value::Array(messages));
	params.insert("stream".into(), json!(true));
	if let Some(key) = clamp_openai_prompt_cache_key(options.and_then(|options| options.session_id.as_deref())) {
		params.insert("prompt_cache_key".into(), json!(key));
	}
	params.insert("store".into(), json!(false));

	if let Some(max_tokens) = options
		.and_then(|options| options.max_tokens)
		.filter(|max_tokens| *max_tokens != 0.0 && !max_tokens.is_nan())
	{
		params.insert(
			"max_output_tokens".into(),
			json!(math_max(max_tokens, OPENAI_RESPONSES_MIN_OUTPUT_TOKENS)),
		);
	}

	if let Some(temperature) = options.and_then(|options| options.temperature) {
		params.insert("temperature".into(), json!(temperature));
	}

	if !transcript_tools.request_tools.is_empty() {
		params.insert(
			"tools".into(),
			Value::Array(convert_responses_tools(
				&transcript_tools.request_tools,
				Some(&tool_options),
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
			let effort = match options.and_then(|options| options.reasoning_effort) {
				Some(effort) => model
					.thinking_level_map
					.as_ref()
					.and_then(|map| map.get(effort.into()))
					.flatten()
					.unwrap_or(effort.as_str())
					.to_string(),
				None => reasoning_effort.as_str().to_string(),
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
			if off != Some(None) {
				params.insert("reasoning".into(), json!({ "effort": off.flatten().unwrap_or("none") }));
			}
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
