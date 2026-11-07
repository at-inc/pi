//! Port of `packages/ai/src/api/bedrock-converse-stream.ts`.
//! PORT-STATUS: reviewed
//!
//! The AWS SDK client is `api::bedrock_client` (see its header for the SDK differences). Differences from the TS:
//! - The command input is JSON, so `onPayload` gets and returns a `Value` and blob members hold base64 text: an image's
//!   `source.bytes` is its data decoded with `atob` and encoded again, and a replayed `redactedContent` is the stored
//!   signature decoded the same way. `onProviderStreamEvent` sees `redactedContent` as base64 text too.
//! - `BedrockOptions` read from `ProviderStreamOptions.extra`: `region`, `profile`, `thinkingDisplay` and
//!   `bearerToken` when they are strings, `interleavedThinking` when it is a boolean, `reasoning` and
//!   `thinkingBudgets` when they parse, `toolChoice` when it is not `null` and `requestMetadata` as given.
//! - HTTP(S) proxies (`resolveHttpProxyUrlForTarget`) and `AWS_BEDROCK_FORCE_HTTP1` select a Node request handler in
//!   the TS. The port always sends through the host's fetch, so neither is read.
//! - The TS checks for a Node or Bun process to pick the region fallbacks; the port always takes the Node branch.
//! - Stream items are the client's `{ [member]: value }` objects. Exception members arriving as events become the
//!   SDK's service exception (`stream_item_exception`), so `formatBedrockError` prefixes them like the TS does.
//! - `contentBlockIndex` keys the open blocks by its JSON text.

use std::cell::{Cell, RefCell};
use std::ops::{Deref, DerefMut};
use std::rc::Rc;
use std::sync::LazyLock;

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use futures::StreamExt;
use indexmap::IndexMap;
use pi_host::js::{math_max, math_min, number_to_string, utf16_len};
use pi_host::{AbortSignal, PiError, PiResult};
use regex::Regex;
use serde_json::{Map, Value, json};

use crate::api::bedrock_client::{
	AwsCredentialIdentity, BedrockRuntimeClient, BedrockRuntimeClientConfig, is_service_exception,
	stream_item_exception,
};
use crate::api::constrained_sampling::{
	get_json_schema_tool_parameters, is_strict_tool_limit_error_message, prefers_json_schema_strict_sampling,
	resolve_json_schema_strict_sampling,
};
use crate::api::openai_codex_images::atob;
use crate::api::simple_options::{
	adjust_max_tokens_for_thinking, build_base_options, clamp_max_tokens_to_context, clamp_reasoning,
};
use crate::api::transform_messages::{split_error_tool_result_images, transform_messages};
use crate::models::calculate_cost;
use crate::types::{
	AssistantContent, AssistantMessage, AssistantMessageEvent, CacheRetention, Message, Model, ModelThinkingLevel,
	ProviderEnv, ProviderResponse, ProviderStreamOptions, ProviderStreams, SharedAssistantMessage, SimpleStreamOptions,
	StopReason, StreamOptions, TextContent, ThinkingBudgets, ThinkingContent, ThinkingLevel, Tool, ToolCall,
	ToolChoice, TranscriptContext, Usage, UserContent, UserMessageContent,
};
use crate::utils::diagnostics::{AssistantMessageDiagnostic, DiagnosticErrorInfo, append_assistant_message_diagnostic};
use crate::utils::error_body::normalize_provider_error;
use crate::utils::event_stream::AssistantMessageEventStream;
use crate::utils::headers::provider_headers_to_record;
use crate::utils::identity::{ClientIdentity, UserAgentMode, UserAgentModeName, get_client_version};
use crate::utils::provider_env::get_provider_env_value;
use crate::utils::sanitize_unicode::sanitize_surrogates;
use crate::utils::streaming_json::ToolCallArgumentsStream;
use crate::utils::text::{get_system_message_text, js_trim};
use crate::utils::transcript::{
	collapse_system_messages, get_current_tools, get_initial_system_message, without_initial_system_message,
};

/// `BedrockOptions`.
#[derive(Clone, Default)]
pub struct BedrockOptions {
	pub base: StreamOptions,
	pub region: Option<String>,
	pub profile: Option<String>,
	/// `"auto" | "any" | "none" | { type: "tool"; name: string }`.
	pub tool_choice: Option<Value>,
	/// See https://docs.aws.amazon.com/bedrock/latest/userguide/inference-reasoning.html for supported models.
	pub reasoning: Option<ThinkingLevel>,
	/// Custom token budgets per thinking level. Overrides default budgets.
	pub thinking_budgets: Option<ThinkingBudgets>,
	/// Only supported by Claude 4.x models, see https://docs.aws.amazon.com/bedrock/latest/userguide/claude-messages-extended-thinking.html#claude-messages-extended-thinking-tool-use-interleaved
	pub interleaved_thinking: Option<bool>,
	/// Controls how Claude's thinking content is returned in responses (`"summarized" | "omitted"`).
	/// - "summarized": Thinking blocks contain summarized thinking text (default here).
	/// - "omitted": Thinking content is redacted but the signature still travels back for multi-turn continuity,
	///   reducing time-to-first-text-token.
	///
	/// Note: Anthropic's API default for Claude Opus 4.8 and Mythos Preview is "omitted". We default to "summarized"
	/// here to keep behavior consistent with older Claude 4 models. Only applies to Claude models on Bedrock.
	pub thinking_display: Option<String>,
	/// Key-value pairs attached to the inference request for cost allocation tagging.
	/// Keys: max 64 chars, no `aws:` prefix. Values: max 256 chars. Max 50 pairs.
	/// Tags appear in AWS Cost Explorer split cost allocation data.
	/// See https://docs.aws.amazon.com/bedrock/latest/APIReference/API_runtime_ConverseStream.html
	pub request_metadata: Option<Value>,
	/// Bearer token for Bedrock API key authentication.
	/// When set, bypasses SigV4 signing and sends `Authorization: Bearer <token>` instead.
	/// Requires `bedrock:CallWithBearerToken` IAM permission on the token's identity.
	/// Set via AWS_BEARER_TOKEN_BEDROCK env var or pass directly.
	/// See https://docs.aws.amazon.com/service-authorization/latest/reference/list_amazonbedrock.html
	pub bearer_token: Option<String>,
}

impl Deref for BedrockOptions {
	type Target = StreamOptions;

	fn deref(&self) -> &Self::Target {
		&self.base
	}
}

impl DerefMut for BedrockOptions {
	fn deref_mut(&mut self) -> &mut Self::Target {
		&mut self.base
	}
}

impl From<ProviderStreamOptions> for BedrockOptions {
	/// Reads the Bedrock options from the extra keys.
	fn from(options: ProviderStreamOptions) -> Self {
		let extra = options.extra;
		let string = |name: &str| extra.get(name).and_then(Value::as_str).map(str::to_string);
		Self {
			base: options.base,
			region: string("region"),
			profile: string("profile"),
			tool_choice: extra.get("toolChoice").filter(|value| !value.is_null()).cloned(),
			reasoning: parsed(&extra, "reasoning"),
			thinking_budgets: parsed(&extra, "thinkingBudgets"),
			interleaved_thinking: extra.get("interleavedThinking").and_then(Value::as_bool),
			thinking_display: string("thinkingDisplay"),
			request_metadata: extra.get("requestMetadata").cloned(),
			bearer_token: string("bearerToken"),
		}
	}
}

/// An extra option read as `T`; a value of another shape is ignored.
fn parsed<T: serde::de::DeserializeOwned>(extra: &Map<String, Value>, name: &str) -> Option<T> {
	serde_json::from_value(extra.get(name)?.clone()).ok()
}

/// Stream state of one open Bedrock content block, keyed by `contentBlockIndex` and kept off the content block so it
/// never reaches partial messages.
struct OpenBlock {
	content_index: usize,
	arguments: Option<ToolCallArgumentsStream>,
	/// Encrypted reasoning deltas, joined into `thinkingSignature` when the block ends.
	redacted_chunks: Option<Vec<u8>>,
}

/// Open blocks by the JSON text of their `contentBlockIndex` (empty when it is missing), in insertion order like a JS
/// `Map`.
type OpenBlocks = IndexMap<String, OpenBlock>;

const EMPTY_TEXT_PLACEHOLDER: &str = "<empty>";

const THINKING_BINDING_CONTROLS_BETA: &str = "thinking-binding-controls-2026-08-01";

/// Matches the placeholder the Anthropic API path uses for redacted thinking.
const REDACTED_THINKING_PLACEHOLDER: &str = "[Reasoning redacted]";

/// `stream(model, context, options)`: streams one Bedrock `ConverseStream` response.
pub fn stream(
	model: &Model,
	context: &TranscriptContext,
	options: Option<BedrockOptions>,
) -> AssistantMessageEventStream {
	let stream = AssistantMessageEventStream::new();
	// Bedrock has no mid-conversation system messages; fold them into the leading prompt.
	let normalized_context = collapse_system_messages(context);
	let options = options.unwrap_or_default();
	let (events, model) = (stream.clone(), model.clone());
	pi_host::spawn_local(async move {
		let output: SharedAssistantMessage = Rc::new(RefCell::new(AssistantMessage {
			content: Vec::new(),
			api: "bedrock-converse-stream".into(),
			provider: model.provider.clone(),
			model: model.id.clone(),
			usage: Usage::default(),
			stop_reason: StopReason::Pending,
			timestamp: pi_host::now_ms(),
			..AssistantMessage::default()
		}));
		let mut open_blocks = OpenBlocks::new();
		let config = client_config(&model, &options);

		// Kept outside the try so the catch can still correlate a mid-stream failure:
		// exceptions delivered as stream events carry no HTTP metadata of their own.
		let mut response_request_id = None;
		let request = Request {
			stream: &events,
			model: &model,
			context: &normalized_context,
			options: &options,
			output: &output,
		};
		let Err(error) = request.run(config, &mut open_blocks, &mut response_request_id).await else {
			return;
		};
		let reason = {
			let mut message = output.borrow_mut();
			flush_open_blocks(&mut open_blocks, &mut message);
			message.stop_reason = if options.signal.as_ref().is_some_and(AbortSignal::aborted) {
				StopReason::Aborted
			} else {
				StopReason::Error
			};
			message.error_message = Some(format_bedrock_error(&error));
			if message.stop_reason == StopReason::Error {
				append_bedrock_failure_diagnostic(&mut message, &error, response_request_id);
			}
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

/// The client config `stream` builds before its `try`.
fn client_config(model: &Model, options: &BedrockOptions) -> BedrockRuntimeClientConfig {
	let env = options.env.as_ref();
	// A profile explicitly configured through pi's auth flow (the `profile`
	// option or scoped `AWS_PROFILE` on the stored credential's env) must win
	// over ambient AWS_ACCESS_KEY_ID/AWS_SECRET_ACCESS_KEY. The SDK default
	// chain already prefers a configured profile over env keys, but only when
	// `credentials` is not set on the client config. See #6957.
	let options_profile =
		truthy(options.profile.clone()).or_else(|| truthy(env.and_then(|env| env.get("AWS_PROFILE")).cloned()));
	let mut config = BedrockRuntimeClientConfig {
		profile: options_profile
			.clone()
			.or_else(|| get_provider_env_value("AWS_PROFILE", env)),
		custom_user_agent: Some(bedrock_user_agent(options.identity.as_ref())),
		..BedrockRuntimeClientConfig::default()
	};
	let configured_region = get_configured_bedrock_region(options);
	let has_ambient_configured_profile = truthy(get_provider_env_value("AWS_PROFILE", None)).is_some();
	let endpoint_region = get_standard_bedrock_endpoint_region(&model.base_url);
	let use_explicit_endpoint = should_use_explicit_bedrock_endpoint(
		&model.base_url,
		configured_region.as_deref(),
		has_ambient_configured_profile,
	);

	// Only pin standard AWS Bedrock runtime endpoints when no region or ambient AWS_PROFILE is configured.
	// This preserves custom endpoints (VPC/proxy) from #3402 without forcing built-in
	// catalog defaults such as us-east-1 to override AWS_REGION/AWS_PROFILE.
	if use_explicit_endpoint {
		config.endpoint = Some(model.base_url.clone());
	}

	// Resolve bearer token for Bedrock API key auth.
	let skip_auth = get_provider_env_value("AWS_BEDROCK_SKIP_AUTH", env).as_deref() == Some("1");
	let bearer_token = truthy(options.bearer_token.clone())
		.or_else(|| truthy(options.api_key.clone()))
		.or_else(|| truthy(get_provider_env_value("AWS_BEARER_TOKEN_BEDROCK", env)));
	let use_bearer_token = bearer_token.is_some() && !skip_auth;

	// Region resolution: ARN-embedded > explicit option > env vars > SDK default chain.
	// When the model ID is an inference profile ARN, extract the region from it.
	// This avoids conflicts with AWS_REGION set for other services.
	if let Some(arn_region) = ARN_REGION.captures(&model.id).and_then(|captures| captures.get(1)) {
		config.region = Some(arn_region.as_str().to_string());
	} else if let Some(region) = &configured_region {
		config.region = Some(region.clone());
	} else if let (Some(region), true) = (&endpoint_region, use_explicit_endpoint) {
		config.region = Some(region.clone());
	} else if !has_ambient_configured_profile {
		config.region = Some("us-east-1".into());
	}

	// Support proxies that don't need authentication
	if skip_auth {
		config.credentials = Some(AwsCredentialIdentity {
			access_key_id: "dummy-access-key".into(),
			secret_access_key: "dummy-secret-key".into(),
			session_token: None,
		});
	}

	if !skip_auth
		&& options_profile.is_none()
		&& let Some(credentials) = get_configured_bedrock_credentials(env)
	{
		config.credentials = Some(credentials);
	}

	if use_bearer_token {
		config.token = bearer_token;
		config.auth_scheme_preference = Some(vec!["httpBearerAuth".into()]);
	}
	config
}

static ARN_REGION: LazyLock<Regex> =
	LazyLock::new(|| Regex::new(r"^arn:aws(?:-[a-z0-9-]+)?:bedrock:([a-z0-9-]+):").expect("valid regex"));

/// `value || undefined` for an optional string.
fn truthy(value: Option<String>) -> Option<String> {
	value.filter(|value| !value.is_empty())
}

/// What the `try` block of `stream` works on.
struct Request<'a> {
	stream: &'a AssistantMessageEventStream,
	model: &'a Model,
	context: &'a TranscriptContext,
	options: &'a BedrockOptions,
	output: &'a SharedAssistantMessage,
}

/// The tool configuration of a request, and whether a tool prefers strict sampling and got it.
struct ToolConfig {
	tool_config: Option<Value>,
	has_preferred_strict_tools: bool,
}

impl Request<'_> {
	/// The `try` block of `stream`.
	async fn run(
		&self,
		config: BedrockRuntimeClientConfig,
		open_blocks: &mut OpenBlocks,
		response_request_id: &mut Option<String>,
	) -> PiResult<()> {
		let Self {
			stream,
			model,
			context,
			options,
			output,
		} = *self;
		let env = options.env.as_ref();
		let supports_strict_mode = model.bedrock_compat().supports_strict_mode.unwrap_or(false);
		let mut client = BedrockRuntimeClient::new(config);
		let observed_raw_response = Rc::new(Cell::new(false));
		if let Some(on_response) = &options.on_response {
			let (on_response, model, observed) = (on_response.clone(), model.clone(), observed_raw_response.clone());
			client.add_deserialize_middleware(Rc::new(move |response: ProviderResponse| {
				observed.set(true);
				on_response(response, &model)
			}));
		}
		if let Some(custom_headers) = provider_headers_to_record(&[options.headers.as_ref()]) {
			client.add_build_middleware(Rc::new(move |request_headers: &mut IndexMap<String, String>| {
				for (key, value) in &custom_headers {
					if !is_reserved_header(key) {
						request_headers.insert(key.clone(), value.clone());
					}
				}
			}));
		}
		let cache_retention = resolve_cache_retention(options.cache_retention, env);
		let inference_max_tokens = options
			.max_tokens
			.or_else(|| is_anthropic_claude_model(model).then_some(model.max_tokens));
		let initial_system_prompt = get_initial_system_message(&context.messages).map(get_system_message_text);
		let mut disable_preferred_strict_tools = false;
		let response = loop {
			let tools = convert_tool_config(
				&get_current_tools(&context.messages),
				options.tool_choice.as_ref(),
				supports_strict_mode,
				disable_preferred_strict_tools,
			)?;
			let mut inference_config = Map::new();
			if let Some(max_tokens) = inference_max_tokens {
				inference_config.insert("maxTokens".into(), json!(max_tokens));
			}
			if let Some(temperature) = options.temperature {
				inference_config.insert("temperature".into(), json!(temperature));
			}
			let mut command_input = Map::new();
			command_input.insert("modelId".into(), json!(model.id));
			command_input.insert(
				"messages".into(),
				Value::Array(convert_messages(context, model, cache_retention, env)?),
			);
			if let Some(system) = build_system_prompt(initial_system_prompt.as_deref(), model, cache_retention, env) {
				command_input.insert("system".into(), system);
			}
			command_input.insert("inferenceConfig".into(), Value::Object(inference_config));
			if let Some(tool_config) = tools.tool_config {
				command_input.insert("toolConfig".into(), tool_config);
			}
			if let Some(fields) = build_additional_model_request_fields(model, options) {
				command_input.insert("additionalModelRequestFields".into(), fields);
			}
			if let Some(request_metadata) = &options.request_metadata {
				command_input.insert("requestMetadata".into(), request_metadata.clone());
			}
			let mut command_input = Value::Object(command_input);
			if let Some(on_payload) = &options.on_payload
				&& let Some(next_command_input) = on_payload(command_input.clone(), model).await?
			{
				command_input = next_command_input;
			}
			let error = match client.converse_stream(&command_input, options.signal.as_ref()).await {
				Ok(response) => break response,
				Err(error) => error,
			};
			// Bedrock checks the tool schemas before it starts the stream, so a retry never repeats streamed output.
			if options.signal.as_ref().is_some_and(AbortSignal::aborted)
				|| disable_preferred_strict_tools
				|| !tools.has_preferred_strict_tools
				|| !is_service_exception(&error)
				|| error.name != "ValidationException"
				|| !is_strict_tool_limit_error_message(&error.message)
			{
				return Err(error);
			}
			disable_preferred_strict_tools = true;
			append_assistant_message_diagnostic(
				&mut output.borrow_mut(),
				AssistantMessageDiagnostic {
					r#type: "bedrock_strict_tools_fallback".into(),
					timestamp: pi_host::now_ms(),
					error: Some(DiagnosticErrorInfo {
						name: None,
						message: error.message.clone(),
						stack: None,
						code: None,
					}),
					details: Some(Map::from_iter([(
						"reason".into(),
						json!("strict_tool_limits_exceeded"),
					)])),
				},
			);
		};
		*response_request_id = normalize_diagnostic_value(response.metadata.request_id.as_deref());
		if !observed_raw_response.get()
			&& let Some(status) = response.metadata.http_status_code
			&& let Some(on_response) = &options.on_response
		{
			let mut headers = IndexMap::new();
			if let Some(request_id) = truthy(response.metadata.request_id.clone()) {
				headers.insert("x-amzn-requestid".to_string(), request_id);
			}
			on_response(ProviderResponse { status, headers }, model).await?;
		}

		let mut items = response.stream;
		while let Some(item) = items.next().await {
			let item = item?;
			if let Some(on_provider_stream_event) = &options.on_provider_stream_event {
				on_provider_stream_event(&item, model).await?;
			}
			self.handle_item(&item, open_blocks)?;
		}

		if options.signal.as_ref().is_some_and(AbortSignal::aborted) {
			return Err(PiError::new("Request was aborted"));
		}

		let (stop_reason, error_message) = {
			let message = output.borrow();
			(message.stop_reason, message.error_message.clone())
		};
		if stop_reason == StopReason::Pending {
			return Err(PiError::new("Bedrock stream ended without a stop reason"));
		}
		if stop_reason == StopReason::Error || stop_reason == StopReason::Aborted {
			return Err(PiError::new(
				truthy(error_message).unwrap_or_else(|| "An unknown error occurred".into()),
			));
		}

		// A stream can settle without stopping every block, so finalize here too.
		flush_open_blocks(open_blocks, &mut output.borrow_mut());
		stream.push(AssistantMessageEvent::Done {
			reason: stop_reason,
			message: output.clone(),
		});
		stream.end(None);
		Ok(())
	}

	/// One `ConverseStreamOutput` item of the `for await` loop.
	fn handle_item(&self, item: &Value, open_blocks: &mut OpenBlocks) -> PiResult<()> {
		if let Some(message_start) = item.get("messageStart") {
			if message_start.get("role").and_then(Value::as_str) != Some("assistant") {
				return Err(PiError::new(
					"Unexpected assistant message start but got user message start instead",
				));
			}
			self.stream.push(AssistantMessageEvent::Start {
				partial: self.output.clone(),
			});
		} else if let Some(event) = item.get("contentBlockStart") {
			self.handle_content_block_start(event, open_blocks);
		} else if let Some(event) = item.get("contentBlockDelta") {
			self.handle_content_block_delta(event, open_blocks);
		} else if let Some(event) = item.get("contentBlockStop") {
			self.handle_content_block_stop(event, open_blocks);
		} else if let Some(event) = item.get("messageStop") {
			let raw_stop_reason = event.get("stopReason").and_then(Value::as_str);
			let (stop_reason, error_message) = map_stop_reason(raw_stop_reason);
			let mut output = self.output.borrow_mut();
			output.raw_stop_reason = raw_stop_reason.map(str::to_string);
			output.stop_reason = stop_reason;
			if let Some(error_message) = error_message {
				output.error_message = Some(error_message);
			}
		} else if let Some(event) = item.get("metadata") {
			handle_metadata(event, self.model, &mut self.output.borrow_mut());
		} else if let Some((member, properties)) = STREAM_EXCEPTION_MEMBERS
			.iter()
			.find_map(|member| item.get(*member).map(|properties| (*member, properties)))
		{
			return Err(stream_item_exception(member, properties)
				.expect("every listed member is an error shape of ConverseStreamOutput"));
		}
		Ok(())
	}

	fn handle_content_block_start(&self, event: &Value, open_blocks: &mut OpenBlocks) {
		let Some(tool_use) = event.get("start").and_then(|start| start.get("toolUse")) else {
			return;
		};
		let text = |name: &str| {
			tool_use
				.get(name)
				.and_then(Value::as_str)
				.unwrap_or_default()
				.to_string()
		};
		let content_index = {
			let mut output = self.output.borrow_mut();
			output.content.push(AssistantContent::ToolCall(ToolCall {
				id: text("toolUseId"),
				name: text("name"),
				..ToolCall::default()
			}));
			output.content.len() - 1
		};
		open_blocks.insert(
			block_key(event),
			OpenBlock {
				content_index,
				arguments: Some(ToolCallArgumentsStream::new()),
				redacted_chunks: None,
			},
		);
		self.stream.push(AssistantMessageEvent::ToolcallStart {
			content_index,
			partial: self.output.clone(),
		});
	}

	fn handle_content_block_delta(&self, event: &Value, open_blocks: &mut OpenBlocks) {
		let key = block_key(event);
		let delta = event.get("delta");
		let member = |name: &str| delta.and_then(|delta| delta.get(name));
		let partial = || self.output.clone();

		if let Some(text) = member("text") {
			let text = text.as_str().unwrap_or_default();
			// If no text block exists yet, create one, as `handleContentBlockStart` is not sent for text blocks
			let content_index = match open_blocks.get(&key) {
				Some(open) => open.content_index,
				None => {
					let content_index = self.push_block(AssistantContent::Text(TextContent::default()));
					open_blocks.insert(key, new_open_block(content_index));
					self.stream.push(AssistantMessageEvent::TextStart {
						content_index,
						partial: partial(),
					});
					content_index
				}
			};
			if let Some(AssistantContent::Text(block)) = self.output.borrow_mut().content.get_mut(content_index) {
				block.text.push_str(text);
			} else {
				return;
			}
			self.stream.push(AssistantMessageEvent::TextDelta {
				content_index,
				delta: text.to_string(),
				partial: partial(),
			});
			return;
		}

		if let Some(tool_use) = member("toolUse")
			&& let Some(open) = open_blocks.get_mut(&key)
			&& let Some(arguments) = &mut open.arguments
		{
			let mut output = self.output.borrow_mut();
			if let Some(AssistantContent::ToolCall(block)) = output.content.get_mut(open.content_index) {
				let input = tool_use.get("input").and_then(Value::as_str).unwrap_or_default();
				arguments.append(input, &mut block.arguments);
				drop(output);
				self.stream.push(AssistantMessageEvent::ToolcallDelta {
					content_index: open.content_index,
					delta: input.to_string(),
					partial: partial(),
				});
				return;
			}
		}

		let Some(reasoning) = member("reasoningContent") else {
			return;
		};
		let content_index = match open_blocks.get(&key) {
			Some(open) => open.content_index,
			None => {
				let content_index = self.push_block(AssistantContent::Thinking(ThinkingContent {
					thinking_signature: Some(String::new()),
					..ThinkingContent::default()
				}));
				open_blocks.insert(key.clone(), new_open_block(content_index));
				self.stream.push(AssistantMessageEvent::ThinkingStart {
					content_index,
					partial: partial(),
				});
				content_index
			}
		};
		let string = |name: &str| {
			reasoning
				.get(name)
				.and_then(Value::as_str)
				.filter(|value| !value.is_empty())
		};
		let mut deltas = Vec::new();
		{
			let mut output = self.output.borrow_mut();
			let Some(AssistantContent::Thinking(block)) = output.content.get_mut(content_index) else {
				return;
			};
			if let Some(text) = string("text") {
				block.thinking.push_str(text);
				deltas.push(text.to_string());
			}
			// `thinkingSignature` holds either an Anthropic signature or an opaque redacted
			// payload, never both: mixing them would corrupt whichever arrived first.
			if let Some(signature) = string("signature")
				&& block.redacted != Some(true)
			{
				block.thinking_signature.get_or_insert_default().push_str(signature);
			}
			if let Some(redacted_content) = string("redactedContent") {
				// Encrypted reasoning from non-Anthropic models on Bedrock (e.g. OpenAI GPT-5.6).
				// The payload is opaque, so keep it verbatim in `thinkingSignature` the way the
				// Anthropic path stores redacted thinking, and replay it on the next turn.
				if block.redacted != Some(true) {
					block.redacted = Some(true);
					block.thinking_signature = Some(String::new());
					block.thinking.push_str(REDACTED_THINKING_PLACEHOLDER);
					deltas.push(REDACTED_THINKING_PLACEHOLDER.to_string());
				}
				// The client hands blobs over as canonical base64, so this decode cannot fail.
				let bytes = STANDARD.decode(redacted_content).unwrap_or_default();
				if let Some(open) = open_blocks.get_mut(&key) {
					open.redacted_chunks.get_or_insert_default().extend(bytes);
				}
			}
		}
		for delta in deltas {
			self.stream.push(AssistantMessageEvent::ThinkingDelta {
				content_index,
				delta,
				partial: partial(),
			});
		}
	}

	fn handle_content_block_stop(&self, event: &Value, open_blocks: &mut OpenBlocks) {
		let Some(mut open) = open_blocks.shift_remove(&block_key(event)) else {
			return;
		};
		let content_index = open.content_index;
		let mut output = self.output.borrow_mut();
		flush_redacted_content(&mut open, &mut output);
		let partial = self.output.clone();
		let event = match &mut output.content[content_index] {
			AssistantContent::Text(block) => AssistantMessageEvent::TextEnd {
				content_index,
				content: block.text.clone(),
				partial,
			},
			AssistantContent::Thinking(block) => AssistantMessageEvent::ThinkingEnd {
				content_index,
				content: block.thinking.clone(),
				partial,
			},
			AssistantContent::ToolCall(block) => {
				if let Some(arguments) = &open.arguments {
					let finished = arguments.finish();
					block.arguments = finished.arguments;
					if finished.arguments_error.is_some() {
						block.arguments_error = finished.arguments_error;
					}
				}
				AssistantMessageEvent::ToolcallEnd {
					content_index,
					tool_call: block.clone(),
					partial,
				}
			}
		};
		drop(output);
		self.stream.push(event);
	}

	/// Appends a content block and returns its index.
	fn push_block(&self, block: AssistantContent) -> usize {
		let mut output = self.output.borrow_mut();
		output.content.push(block);
		output.content.len() - 1
	}
}

/// The exception members of `ConverseStreamOutput` that the `for await` loop throws, in its order.
const STREAM_EXCEPTION_MEMBERS: [&str; 5] = [
	"internalServerException",
	"modelStreamErrorException",
	"validationException",
	"throttlingException",
	"serviceUnavailableException",
];

/// The `openBlocks` key of an event: its `contentBlockIndex` as JSON text.
fn block_key(event: &Value) -> String {
	event.get("contentBlockIndex").map(Value::to_string).unwrap_or_default()
}

fn new_open_block(content_index: usize) -> OpenBlock {
	OpenBlock {
		content_index,
		arguments: None,
		redacted_chunks: None,
	}
}

/// Encodes buffered encrypted reasoning into `thinkingSignature`. The chunks stay in the stream state, never on the
/// block.
fn flush_redacted_content(open: &mut OpenBlock, output: &mut AssistantMessage) {
	let Some(AssistantContent::Thinking(block)) = output.content.get_mut(open.content_index) else {
		return;
	};
	let Some(chunks) = open.redacted_chunks.take() else {
		return;
	};
	block.thinking_signature = Some(STANDARD.encode(chunks));
}

/// Ends the stream state of blocks a stream settled without stopping.
fn flush_open_blocks(open_blocks: &mut OpenBlocks, output: &mut AssistantMessage) {
	for open in open_blocks.values_mut() {
		flush_redacted_content(open, output);
	}
	open_blocks.clear();
}

fn handle_metadata(event: &Value, model: &Model, output: &mut AssistantMessage) {
	let Some(usage) = event.get("usage") else {
		return;
	};
	let count = |name: &str| {
		usage
			.get(name)
			.and_then(Value::as_f64)
			.filter(|count| *count != 0.0 && !count.is_nan())
			.unwrap_or(0.0)
	};
	output.usage.input = count("inputTokens");
	output.usage.output = count("outputTokens");
	output.usage.cache_read = count("cacheReadInputTokens");
	output.usage.cache_write = count("cacheWriteInputTokens");
	output.usage.cache_write_1h = usage.get("cacheDetails").and_then(Value::as_array).map(|details| {
		details.iter().fold(0.0, |total, detail| {
			total
				+ if detail.get("ttl").and_then(Value::as_str) == Some("1h") {
					detail.get("inputTokens").and_then(Value::as_f64).unwrap_or(0.0)
				} else {
					0.0
				}
		})
	});
	let total_tokens = count("totalTokens");
	output.usage.total_tokens = if total_tokens != 0.0 {
		total_tokens
	} else {
		output.usage.input + output.usage.output
	};
	calculate_cost(model, &mut output.usage);
}

/// Human-readable prefixes for Bedrock SDK exception names.
/// The downstream retry logic in agent-session matches patterns like
/// `server.?error` and `service.?unavailable`, so we preserve the legacy
/// prefix format rather than using the raw SDK exception name.
const BEDROCK_ERROR_PREFIXES: [(&str, &str); 5] = [
	("InternalServerException", "Internal server error"),
	("ModelStreamErrorException", "Model stream error"),
	("ValidationException", "Validation error"),
	("ThrottlingException", "Throttling error"),
	("ServiceUnavailableException", "Service unavailable"),
];

/// Some models reject the account/profile's configured Bedrock data retention mode
/// (e.g. "data retention mode 'default' is not available for this model"). Point
/// users at the AWS docs explaining how to configure a supported mode.
const BEDROCK_DATA_RETENTION_DOCS_URL: &str =
	"https://docs.aws.amazon.com/bedrock/latest/userguide/data-retention.html";

static DATA_RETENTION_MODE: LazyLock<Regex> =
	LazyLock::new(|| Regex::new(r"(?i-u)data retention mode").expect("valid regex"));

/// Format a Bedrock error with a human-readable prefix.
/// AWS SDK exceptions (both from `client.send()` and from stream event items)
/// extend BedrockRuntimeServiceException. We map the `.name` to a stable
/// human-readable prefix so downstream consumers (retry logic, context-overflow
/// detection) can distinguish error categories via simple string matching.
fn format_bedrock_error(error: &PiError) -> String {
	let norm = normalize_provider_error(error);
	// Surface the raw HTTP body (with status) when the SDK did not fold it into
	// the message; otherwise fall back to the message. This is what stops a
	// gateway 403 from collapsing to `Unknown: UnknownError`.
	let core = match (norm.status, &norm.body) {
		(Some(status), Some(body)) if !norm.message_carries_body => format!("{}: {body}", number_to_string(status)),
		_ => norm.message,
	};
	let data_retention_hint = if DATA_RETENTION_MODE.is_match(&core) {
		format!(" See {BEDROCK_DATA_RETENTION_DOCS_URL} for supported data retention modes.")
	} else {
		String::new()
	};
	if is_service_exception(error) {
		let prefix = BEDROCK_ERROR_PREFIXES
			.iter()
			.find(|(name, _)| *name == error.name)
			.map_or(error.name.as_str(), |(_, prefix)| prefix);
		return format!("{prefix}: {core}{data_retention_hint}");
	}
	format!("{core}{data_retention_hint}")
}

/// Over-long header values are dropped rather than truncated: a truncated request id is not a request id.
const MAX_BEDROCK_DIAGNOSTIC_VALUE_CHARS: usize = 200;

fn normalize_diagnostic_value(value: Option<&str>) -> Option<String> {
	let trimmed = js_trim(value?);
	if trimmed.is_empty() || utf16_len(trimmed) > MAX_BEDROCK_DIAGNOSTIC_VALUE_CHARS {
		return None;
	}
	Some(trimmed.to_string())
}

/// The SDK puts the modeled code on `error.name` for service exceptions and unmodeled stream errors alike, so
/// do not narrow to `BedrockRuntimeServiceException`. Modeled Bedrock errors all end in `Exception`, unlike
/// transport names such as `TimeoutError`.
fn extract_bedrock_error_code(error: &PiError) -> Option<String> {
	if !error.name.ends_with("Exception") {
		return None;
	}
	normalize_diagnostic_value(Some(&error.name))
}

/// Structured metadata alongside `errorMessage`, which stays byte-identical because `isRetryableAssistantError`
/// matches against it. Unknown fields are omitted, never guessed: a modeled mid-stream exception carries no
/// `$metadata`, leaving only `fallbackRequestId`.
fn append_bedrock_failure_diagnostic(
	output: &mut AssistantMessage,
	error: &PiError,
	fallback_request_id: Option<String>,
) {
	let metadata = error.data.as_ref().and_then(|data| data.get("$metadata"));
	let mut details = Map::new();

	if let Some(status) = metadata
		.and_then(|metadata| metadata.get("httpStatusCode"))
		.filter(|status| status.is_number())
	{
		details.insert("status".into(), status.clone());
	}

	if let Some(error_code) = extract_bedrock_error_code(error) {
		details.insert("errorCode".into(), json!(error_code));
	}

	let request_id = normalize_diagnostic_value(
		metadata
			.and_then(|metadata| metadata.get("requestId"))
			.and_then(Value::as_str),
	)
	.or(fallback_request_id);
	if let Some(request_id) = request_id {
		details.insert("requestId".into(), json!(request_id));
	}

	if details.is_empty() {
		return;
	}

	append_assistant_message_diagnostic(
		output,
		AssistantMessageDiagnostic {
			r#type: "bedrock_response_failure".into(),
			timestamp: pi_host::now_ms(),
			error: None,
			details: Some(details),
		},
	);
}

/// Header keys that must never be overwritten by caller-supplied headers.
/// `host` and `x-amz-*` participate in the SigV4 canonical request; `authorization`
/// is owned by SigV4 or the bearer-token path (config.token + authSchemePreference).
/// Compared case-insensitively (caller key is lower-cased before lookup).
fn is_reserved_header(key: &str) -> bool {
	let lower = key.to_lowercase();
	lower.starts_with("x-amz-") || lower == "authorization" || lower == "host"
}

/// The client identity as AWS SDK user agent pairs. The SDK appends them to its own `User-Agent` and escapes characters
/// that are not valid in a user agent token, so the detailed form is reduced to name and version.
fn bedrock_user_agent(identity: Option<&ClientIdentity>) -> Vec<(String, Option<String>)> {
	match identity.and_then(|identity| identity.user_agent.clone()) {
		Some(UserAgentMode::Custom { custom }) => vec![(custom, None)],
		Some(UserAgentMode::Named(UserAgentModeName::Short)) => vec![("pi".into(), None)],
		_ => vec![("pi".into(), Some(get_client_version(identity)))],
	}
}

/// `streamSimple(model, context, options)`: maps the simple options to `BedrockOptions`.
pub fn stream_simple(
	model: &Model,
	context: &TranscriptContext,
	options: Option<SimpleStreamOptions>,
) -> AssistantMessageEventStream {
	let base = BedrockOptions {
		base: build_base_options(model, context, options.as_ref(), None),
		tool_choice: options
			.as_ref()
			.and_then(|options| options.tool_choice)
			.map(|tool_choice| {
				json!(match tool_choice {
					ToolChoice::Auto => "auto",
					ToolChoice::None => "none",
				})
			}),
		..BedrockOptions::default()
	};
	let Some(reasoning) = options.as_ref().and_then(|options| options.reasoning) else {
		return stream(model, context, Some(base));
	};
	let thinking_budgets = options.and_then(|options| options.thinking_budgets);

	if is_anthropic_claude_model(model) {
		if supports_adaptive_thinking(&model.id, &model.name) {
			return stream(
				model,
				context,
				Some(BedrockOptions {
					reasoning: Some(reasoning),
					thinking_budgets,
					..base
				}),
			);
		}

		// Undefined means the caller did not request an output cap; let the helper use the model cap.
		// Do not coerce to 0 here, or the thinking budget would become the entire maxTokens value.
		let adjusted =
			adjust_max_tokens_for_thinking(base.max_tokens, model.max_tokens, reasoning, thinking_budgets.as_ref());

		let max_tokens = clamp_max_tokens_to_context(model, context, adjusted.max_tokens);

		let mut budgets = thinking_budgets.unwrap_or_default();
		let budget = Some(math_min(adjusted.thinking_budget, math_max(0.0, max_tokens - 1024.0)));
		match clamp_reasoning(Some(reasoning)) {
			Some(ThinkingLevel::Minimal) => budgets.minimal = budget,
			Some(ThinkingLevel::Low) => budgets.low = budget,
			Some(ThinkingLevel::Medium) => budgets.medium = budget,
			_ => budgets.high = budget,
		}
		let mut base = base;
		base.max_tokens = Some(max_tokens);
		return stream(
			model,
			context,
			Some(BedrockOptions {
				reasoning: Some(reasoning),
				thinking_budgets: Some(budgets),
				..base
			}),
		);
	}

	stream(
		model,
		context,
		Some(BedrockOptions {
			reasoning: Some(reasoning),
			thinking_budgets,
			..base
		}),
	)
}

/// Check if the model supports adaptive thinking (Opus 4.6+, Sonnet 4.6).
/// Checks both model ID and model name to support application inference profiles
/// whose ARNs don't contain the model name.
fn get_model_match_candidates(model_id: &str, model_name: &str) -> Vec<String> {
	let values: &[&str] = if model_name.is_empty() {
		&[model_id]
	} else {
		&[model_id, model_name]
	};
	values
		.iter()
		.flat_map(|value| {
			let lower = value.to_lowercase();
			let dashed = MATCH_SEPARATORS.replace_all(&lower, "-").into_owned();
			[lower, dashed]
		})
		.collect()
}

/// JS `/[\s_.:]+/g`: ECMAScript white space and line terminators, underscore, dot and colon.
static MATCH_SEPARATORS: LazyLock<Regex> = LazyLock::new(|| {
	Regex::new(r"[\t\n\x0B\x0C\r \x{A0}\x{1680}\x{2000}-\x{200A}\x{2028}\x{2029}\x{202F}\x{205F}\x{3000}\x{FEFF}_.:]+")
		.expect("valid regex")
});

fn matches_any(model_id: &str, model_name: &str, needles: &[&str]) -> bool {
	get_model_match_candidates(model_id, model_name)
		.iter()
		.any(|candidate| needles.iter().any(|needle| candidate.contains(needle)))
}

fn supports_adaptive_thinking(model_id: &str, model_name: &str) -> bool {
	matches_any(
		model_id,
		model_name,
		&[
			"opus-4-6",
			"opus-4-7",
			"opus-4-8",
			"opus-5",
			"sonnet-4-6",
			"sonnet-5",
			"haiku-5",
			"fable-5",
		],
	)
}

fn supports_native_xhigh_effort(model: &Model) -> bool {
	matches_any(
		&model.id,
		&model.name,
		&["opus-4-7", "opus-4-8", "opus-5", "sonnet-5", "haiku-5", "fable-5"],
	)
}

/// Check if the model accepts `thinking.block_binding`. Opus 4.6 and Sonnet 4.6 reject it with
/// "thinking.adaptive.block_binding: Extra inputs are not permitted".
fn supports_thinking_block_binding(model: &Model) -> bool {
	matches_any(
		&model.id,
		&model.name,
		&["opus-4-7", "opus-4-8", "opus-5", "sonnet-5", "haiku-5", "fable-5"],
	)
}

fn map_thinking_level_to_effort(model: &Model, level: ThinkingLevel) -> String {
	if level == ThinkingLevel::Xhigh && supports_native_xhigh_effort(model) {
		return "xhigh".into();
	}

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
		_ => "high",
	}
	.into()
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

/// Check if the model is an Anthropic Claude model on Bedrock.
/// Checks both model ID and model name to support application inference profiles
/// whose ARNs don't contain the model name.
fn is_anthropic_claude_model(model: &Model) -> bool {
	let id = model.id.to_lowercase();
	let name = model.name.to_lowercase();
	id.contains("anthropic.claude")
		|| id.contains("anthropic/claude")
		|| name.contains("anthropic.claude")
		|| name.contains("anthropic/claude")
		|| name.contains("claude")
}

/// Check if the model supports prompt caching.
/// Supported: Claude 3.5 Haiku, Claude 3.7 Sonnet, Claude 4.x models, Claude 5 models
///
/// For base models and system-defined inference profiles the model ID / ARN
/// contains the model name, so we can decide locally.
///
/// For application inference profiles (whose ARNs don't contain the model name),
/// also checks model.name which is user-controlled via models.json or registerProvider.
/// As a last resort, set AWS_BEDROCK_FORCE_CACHE=1 to enable cache points.
/// Amazon Nova models have automatic caching and don't need explicit cache points.
fn supports_prompt_caching(model: &Model, env: Option<&ProviderEnv>) -> bool {
	let candidates = get_model_match_candidates(&model.id, &model.name);
	let any = |needles: &[&str]| {
		candidates
			.iter()
			.any(|candidate| needles.iter().any(|needle| candidate.contains(needle)))
	};

	if !any(&["claude"]) {
		// Application inference profiles don't contain the model name in the ARN.
		// Allow users to force cache points via environment variable.
		return get_provider_env_value("AWS_BEDROCK_FORCE_CACHE", env).as_deref() == Some("1");
	}
	// Claude 5 models (fable-5, opus-5, sonnet-5, haiku-5)
	// Claude 4.x models (opus-4, sonnet-4, haiku-4)
	// Claude 3.7 Sonnet
	// Claude 3.5 Haiku
	any(&["fable-5", "opus-5", "sonnet-5", "haiku-5"])
		|| any(&["-4-"])
		|| any(&["claude-3-7-sonnet"])
		|| any(&["claude-3-5-haiku"])
}

/// Check if the model supports thinking signatures in reasoningContent.
/// Only Anthropic Claude models support the signature field.
/// Other models (OpenAI, Qwen, Minimax, Moonshot, etc.) reject it with:
/// "This model doesn't support the reasoningContent.reasoningText.signature field"
///
/// Checks both model ID and model name to support application inference profiles.
fn supports_thinking_signature(model: &Model) -> bool {
	is_anthropic_claude_model(model)
}

/// The `cachePoint` block for a cache retention other than `none`.
fn cache_point(cache_retention: CacheRetention) -> Value {
	let mut cache_point = Map::from_iter([("type".to_string(), json!("default"))]);
	if cache_retention == CacheRetention::Long {
		cache_point.insert("ttl".into(), json!("1h"));
	}
	json!({ "cachePoint": cache_point })
}

fn build_system_prompt(
	system_prompt: Option<&str>,
	model: &Model,
	cache_retention: CacheRetention,
	env: Option<&ProviderEnv>,
) -> Option<Value> {
	let system_prompt = system_prompt.filter(|prompt| !prompt.is_empty())?;

	let mut blocks = vec![json!({ "text": sanitize_surrogates(system_prompt) })];

	// Add cache point for supported Claude models when caching is enabled
	if cache_retention != CacheRetention::None && supports_prompt_caching(model, env) {
		blocks.push(cache_point(cache_retention));
	}

	Some(Value::Array(blocks))
}

/// JS `id.replace(/[^a-zA-Z0-9_-]/g, "_")` then `slice(0, 64)`: the regex has no `u` flag, so a character outside
/// the BMP is two UTF-16 code units and becomes two underscores.
fn normalize_tool_call_id(id: &str) -> String {
	let mut sanitized = String::with_capacity(id.len());
	for ch in id.chars() {
		if ch.is_ascii_alphanumeric() || ch == '_' || ch == '-' {
			sanitized.push(ch);
		} else {
			sanitized.extend(std::iter::repeat_n('_', ch.len_utf16()));
		}
	}
	sanitized.truncate(64);
	sanitized
}

fn create_non_blank_text_block(text: &str) -> Option<Value> {
	let sanitized = sanitize_surrogates(text);
	if js_trim(sanitized).is_empty() {
		return None;
	}
	Some(json!({ "text": sanitized }))
}

fn create_required_text_block(text: &str) -> Value {
	create_non_blank_text_block(text).unwrap_or_else(|| json!({ "text": EMPTY_TEXT_PLACEHOLDER }))
}

fn sanitize_bedrock_document(value: &Value) -> Value {
	match value {
		Value::Array(items) => Value::Array(items.iter().map(sanitize_bedrock_document).collect()),
		Value::Object(members) => Value::Object(
			members
				.iter()
				.filter(|(key, _)| !key.is_empty())
				.map(|(key, nested)| (key.clone(), sanitize_bedrock_document(nested)))
				.collect(),
		),
		value => value.clone(),
	}
}

fn convert_tool_result_content(content: Vec<UserContent>) -> PiResult<Vec<Value>> {
	let mut result = Vec::new();
	for c in content {
		match c {
			UserContent::Image(image) => {
				result.push(json!({ "image": create_image_block(&image.mime_type, &image.data)? }))
			}
			UserContent::Text(text) => result.extend(create_non_blank_text_block(&text.text)),
		}
	}
	if result.is_empty() {
		result.push(json!({ "text": EMPTY_TEXT_PLACEHOLDER }));
	}
	Ok(result)
}

fn convert_messages(
	context: &TranscriptContext,
	model: &Model,
	cache_retention: CacheRetention,
	env: Option<&ProviderEnv>,
) -> PiResult<Vec<Value>> {
	let mut result: Vec<Value> = Vec::new();
	let transformed_messages = transform_messages(
		without_initial_system_message(&context.messages),
		model,
		Some(&|id: &str, _model: &Model, _source: &AssistantMessage| normalize_tool_call_id(id)),
	);

	let mut i = 0;
	while i < transformed_messages.len() {
		match &transformed_messages[i] {
			Message::User(m) => {
				let mut content = Vec::new();
				match &m.content {
					UserMessageContent::Text(text) => content.push(create_required_text_block(text)),
					UserMessageContent::Blocks(blocks) => {
						for c in blocks {
							match c {
								UserContent::Text(text) => content.extend(create_non_blank_text_block(&text.text)),
								UserContent::Image(image) => {
									content.push(json!({ "image": create_image_block(&image.mime_type, &image.data)? }))
								}
							}
						}
						if content.is_empty() {
							content.push(json!({ "text": EMPTY_TEXT_PLACEHOLDER }));
						}
					}
				}
				result.push(json!({ "role": "user", "content": content }));
			}
			Message::Assistant(m) => {
				// Skip assistant messages with empty content (e.g., from aborted requests)
				// Bedrock rejects messages with empty content arrays
				if !m.content.is_empty() {
					let content_blocks = convert_assistant_content(&m.content, model);
					// Skip if all content blocks were filtered out
					if !content_blocks.is_empty() {
						result.push(json!({ "role": "assistant", "content": content_blocks }));
					}
				}
			}
			Message::ToolResult(_) => {
				// Collect all consecutive toolResult messages into a single user message
				// Bedrock requires all tool results to be in one message
				let mut tool_results = Vec::new();
				// Claude rejects error tool results that contain anything but text, so for Claude their
				// images follow all tool results of the turn (#10794). Other models keep them in place.
				let mut error_images = Vec::new();
				let mut j = i;
				while let Some(Message::ToolResult(tool_msg)) = transformed_messages.get(j) {
					let split = if is_anthropic_claude_model(model) {
						split_error_tool_result_images(tool_msg)
					} else {
						None
					};
					let content = match &split {
						Some(split) => split.content.iter().cloned().map(UserContent::Text).collect(),
						None => tool_msg.content.clone(),
					};
					tool_results.push(json!({
						"toolResult": {
							"toolUseId": tool_msg.tool_call_id,
							"content": convert_tool_result_content(content)?,
							"status": if tool_msg.is_error { "error" } else { "success" },
						},
					}));
					if let Some(split) = split {
						error_images.push(json!({ "text": split.caption }));
						for image in &split.images {
							error_images.push(json!({ "image": create_image_block(&image.mime_type, &image.data)? }));
						}
					}
					j += 1;
				}

				// Skip the messages we've already processed
				i = j - 1;

				tool_results.extend(error_images);
				result.push(json!({ "role": "user", "content": tool_results }));
			}
			Message::System(_) => {}
		}
		i += 1;
	}

	// Add cache point to the last user message for supported Claude models when caching is enabled
	if cache_retention != CacheRetention::None
		&& supports_prompt_caching(model, env)
		&& let Some(last_message) = result.last_mut()
		&& last_message["role"] == "user"
		&& let Some(Value::Array(content)) = last_message.get_mut("content")
	{
		content.push(cache_point(cache_retention));
	}

	Ok(result)
}

/// The content blocks of one assistant message.
fn convert_assistant_content(content: &[AssistantContent], model: &Model) -> Vec<Value> {
	let mut content_blocks = Vec::new();
	for c in content {
		match c {
			AssistantContent::Text(text) => {
				// Skip empty text blocks
				content_blocks.extend(create_non_blank_text_block(&text.text));
			}
			AssistantContent::ToolCall(tool_call) => content_blocks.push(json!({
				"toolUse": {
					"toolUseId": tool_call.id,
					"name": tool_call.name,
					"input": sanitize_bedrock_document(&Value::Object(tool_call.arguments.clone())),
				},
			})),
			AssistantContent::Thinking(thinking) => {
				// Encrypted reasoning is opaque: replay the stored payload as the
				// `redactedContent` member instead of lowering it to reasoning text.
				if thinking.redacted == Some(true) {
					if let Some(redacted_content) = decode_redacted_content(thinking.thinking_signature.as_deref())
						&& !redacted_content.is_empty()
					{
						content_blocks.push(json!({
							"reasoningContent": { "redactedContent": STANDARD.encode(redacted_content) },
						}));
					}
					continue;
				}
				// Skip empty thinking blocks
				let text = sanitize_surrogates(&thinking.thinking);
				if js_trim(text).is_empty() {
					continue;
				}
				// Only Anthropic models support the signature field in reasoningText.
				// For other models, we omit the signature to avoid errors like:
				// "This model doesn't support the reasoningContent.reasoningText.signature field"
				if !supports_thinking_signature(model) {
					content_blocks.push(json!({ "reasoningContent": { "reasoningText": { "text": text } } }));
					continue;
				}
				// Signatures arrive after thinking deltas. If a partial or externally
				// persisted message lacks a signature, Bedrock rejects the replayed
				// reasoning block. Fall back to plain text, matching Anthropic.
				match thinking
					.thinking_signature
					.as_deref()
					.filter(|signature| !js_trim(signature).is_empty())
				{
					None => content_blocks.push(json!({ "text": text })),
					Some(signature) => content_blocks.push(json!({
						"reasoningContent": { "reasoningText": { "text": text, "signature": signature } },
					})),
				}
			}
		}
	}
	content_blocks
}

fn convert_tool_config(
	tools: &[Tool],
	tool_choice: Option<&Value>,
	supports_strict_mode: bool,
	disable_preferred_strict_tools: bool,
) -> PiResult<ToolConfig> {
	if tools.is_empty() || tool_choice.and_then(Value::as_str) == Some("none") {
		return Ok(ToolConfig {
			tool_config: None,
			has_preferred_strict_tools: false,
		});
	}

	let mut has_preferred_strict_tools = false;
	let mut bedrock_tools = Vec::with_capacity(tools.len());
	for tool in tools {
		let preferred = prefers_json_schema_strict_sampling(tool);
		let strict = if disable_preferred_strict_tools && preferred {
			None
		} else {
			resolve_json_schema_strict_sampling(tool, supports_strict_mode, None)?
		};
		if strict == Some(true) && preferred {
			has_preferred_strict_tools = true;
		}
		let mut tool_spec = Map::new();
		tool_spec.insert("name".into(), json!(tool.name));
		tool_spec.insert("description".into(), json!(tool.description));
		tool_spec.insert(
			"inputSchema".into(),
			json!({ "json": get_json_schema_tool_parameters(tool, strict)? }),
		);
		if strict == Some(true) {
			tool_spec.insert("strict".into(), Value::Bool(true));
		}
		bedrock_tools.push(json!({ "toolSpec": tool_spec }));
	}

	let bedrock_tool_choice = match tool_choice {
		Some(Value::String(choice)) if choice == "auto" => Some(json!({ "auto": {} })),
		Some(Value::String(choice)) if choice == "any" => Some(json!({ "any": {} })),
		Some(choice) if choice.get("type").and_then(Value::as_str) == Some("tool") => {
			let mut tool = Map::new();
			if let Some(name) = choice.get("name") {
				tool.insert("name".into(), name.clone());
			}
			Some(json!({ "tool": tool }))
		}
		_ => None,
	};

	let mut tool_config = Map::from_iter([("tools".to_string(), Value::Array(bedrock_tools))]);
	if let Some(bedrock_tool_choice) = bedrock_tool_choice {
		tool_config.insert("toolChoice".into(), bedrock_tool_choice);
	}
	Ok(ToolConfig {
		tool_config: Some(Value::Object(tool_config)),
		has_preferred_strict_tools,
	})
}

fn map_stop_reason(reason: Option<&str>) -> (StopReason, Option<String>) {
	match reason {
		Some("end_turn" | "stop_sequence") => (StopReason::Stop, None),
		Some("max_tokens" | "model_context_window_exceeded") => (StopReason::Length, None),
		Some("tool_use") => (StopReason::ToolUse, None),
		Some(reason) if !reason.is_empty() => (StopReason::Error, Some(format!("Provider stopped with: {reason}"))),
		_ => (StopReason::Error, None),
	}
}

fn get_configured_bedrock_region(options: &BedrockOptions) -> Option<String> {
	let env = options.env.as_ref();
	truthy(options.region.clone())
		.or_else(|| truthy(get_provider_env_value("AWS_REGION", env)))
		.or_else(|| truthy(get_provider_env_value("AWS_DEFAULT_REGION", env)))
}

fn get_configured_bedrock_credentials(env: Option<&ProviderEnv>) -> Option<AwsCredentialIdentity> {
	let access_key_id = truthy(get_provider_env_value("AWS_ACCESS_KEY_ID", env))?;
	let secret_access_key = truthy(get_provider_env_value("AWS_SECRET_ACCESS_KEY", env))?;
	Some(AwsCredentialIdentity {
		access_key_id,
		secret_access_key,
		session_token: truthy(get_provider_env_value("AWS_SESSION_TOKEN", env)),
	})
}

fn get_standard_bedrock_endpoint_region(base_url: &str) -> Option<String> {
	if base_url.is_empty() {
		return None;
	}
	let url = url::Url::parse(base_url).ok()?;
	let hostname = url.host_str()?.to_lowercase();
	STANDARD_ENDPOINT
		.captures(&hostname)
		.and_then(|captures| captures.get(1))
		.map(|region| region.as_str().to_string())
}

static STANDARD_ENDPOINT: LazyLock<Regex> = LazyLock::new(|| {
	Regex::new(r"^bedrock-runtime(?:-fips)?\.([a-z0-9-]+)\.amazonaws\.com(?:\.cn)?$").expect("valid regex")
});

fn should_use_explicit_bedrock_endpoint(
	base_url: &str,
	configured_region: Option<&str>,
	has_ambient_configured_profile: bool,
) -> bool {
	if get_standard_bedrock_endpoint_region(base_url).is_none() {
		return true;
	}

	configured_region.is_none() && !has_ambient_configured_profile
}

fn is_gov_cloud_bedrock_target(model: &Model, options: &BedrockOptions) -> bool {
	if get_configured_bedrock_region(options).is_some_and(|region| region.to_lowercase().starts_with("us-gov-")) {
		return true;
	}

	let model_id = model.id.to_lowercase();
	model_id.starts_with("us-gov.") || model_id.starts_with("arn:aws-us-gov:")
}

fn build_additional_model_request_fields(model: &Model, options: &BedrockOptions) -> Option<Value> {
	let reasoning = options.reasoning.filter(|_| model.reasoning)?;

	if is_anthropic_claude_model(model) {
		// GovCloud Bedrock currently rejects the Claude thinking.display field.
		// Omit it there until the GovCloud Converse schema catches up.
		let is_gov_cloud = is_gov_cloud_bedrock_target(model, options);
		let display = (!is_gov_cloud).then(|| options.thinking_display.clone().unwrap_or_else(|| "summarized".into()));
		// Replayed signed thinking blocks are bound to the system prompt and tools they were
		// created with. Bedrock 400s on replay after either changes unless stale blocks are
		// dropped, matching the Anthropic provider. Skipped on GovCloud like display.
		let use_block_binding = !is_gov_cloud && supports_thinking_block_binding(model);
		let adaptive = supports_adaptive_thinking(&model.id, &model.name);
		let mut thinking = Map::new();
		let mut result = Map::new();
		if adaptive {
			thinking.insert("type".into(), json!("adaptive"));
			if let Some(display) = display {
				thinking.insert("display".into(), json!(display));
			}
			if use_block_binding {
				thinking.insert(
					"block_binding".into(),
					json!({ "prefix_mismatch_behavior": "drop_block" }),
				);
			}
			result.insert("thinking".into(), Value::Object(thinking));
			result.insert(
				"output_config".into(),
				json!({ "effort": map_thinking_level_to_effort(model, reasoning) }),
			);
			if use_block_binding {
				result.insert("anthropic_beta".into(), json!([THINKING_BINDING_CONTROLS_BETA]));
			}
		} else {
			// Custom budgets only cover token-based levels through high.
			let budgets = options.thinking_budgets.as_ref();
			let custom = match reasoning {
				ThinkingLevel::Minimal => budgets.and_then(|budgets| budgets.minimal),
				ThinkingLevel::Low => budgets.and_then(|budgets| budgets.low),
				ThinkingLevel::Medium => budgets.and_then(|budgets| budgets.medium),
				ThinkingLevel::High | ThinkingLevel::Xhigh | ThinkingLevel::Max => {
					budgets.and_then(|budgets| budgets.high)
				}
			};
			let budget = custom.unwrap_or(match reasoning {
				ThinkingLevel::Minimal => 1024.0,
				ThinkingLevel::Low => 2048.0,
				ThinkingLevel::Medium => 8192.0,
				// Budget-based Claude clamps extended levels to high
				ThinkingLevel::High | ThinkingLevel::Xhigh | ThinkingLevel::Max => 16384.0,
			});
			thinking.insert("type".into(), json!("enabled"));
			thinking.insert("budget_tokens".into(), json!(budget));
			if let Some(display) = display {
				thinking.insert("display".into(), json!(display));
			}
			result.insert("thinking".into(), Value::Object(thinking));
		}

		if !adaptive && options.interleaved_thinking.unwrap_or(true) {
			result.insert("anthropic_beta".into(), json!(["interleaved-thinking-2025-05-14"]));
		}

		return Some(Value::Object(result));
	}

	let candidates = get_model_match_candidates(&model.id, &model.name);

	if candidates.iter().any(|candidate| candidate.contains("gpt-oss")) {
		return Some(json!({ "reasoning_effort": openai_gpt_oss_effort(reasoning) }));
	}

	if candidates.iter().any(|candidate| candidate.contains("gpt-")) {
		let effort = match model
			.thinking_level_map
			.as_ref()
			.and_then(|map| map.get(ModelThinkingLevel::from(reasoning)))
		{
			Some(Some(mapped)) => mapped.to_string(),
			_ => openai_gpt_effort(reasoning).to_string(),
		};
		return Some(json!({ "reasoning": { "effort": effort } }));
	}

	None
}

/// OpenAI GPT models (GPT-5.x, GPT-6) take a nested `reasoning.effort` and reject `minimal`.
fn openai_gpt_effort(level: ThinkingLevel) -> &'static str {
	match level {
		ThinkingLevel::Minimal | ThinkingLevel::Low => "low",
		ThinkingLevel::Medium => "medium",
		ThinkingLevel::High => "high",
		ThinkingLevel::Xhigh => "xhigh",
		ThinkingLevel::Max => "max",
	}
}

/// gpt-oss takes a flat `reasoning_effort` and only accepts low, medium and high.
fn openai_gpt_oss_effort(level: ThinkingLevel) -> &'static str {
	match level {
		ThinkingLevel::Minimal | ThinkingLevel::Low => "low",
		ThinkingLevel::Medium => "medium",
		ThinkingLevel::High | ThinkingLevel::Xhigh | ThinkingLevel::Max => "high",
	}
}

/// `createImageBlock(mimeType, data)`: the bytes as base64 text, decoded with `atob` and encoded again like the SDK
/// encodes the `Uint8Array`.
fn create_image_block(mime_type: &str, data: &str) -> PiResult<Value> {
	let format = match mime_type {
		"image/jpeg" | "image/jpg" => "jpeg",
		"image/png" => "png",
		"image/gif" => "gif",
		"image/webp" => "webp",
		_ => return Err(PiError::new(format!("Unknown image type: {mime_type}"))),
	};

	let bytes = base64_to_bytes(data).ok_or_else(|| {
		PiError::named(
			"InvalidCharacterError",
			"The string to be decoded is not correctly encoded.",
		)
	})?;
	Ok(json!({ "source": { "bytes": STANDARD.encode(bytes) }, "format": format }))
}

/// `base64ToBytes(data)`; `None` where `atob` throws.
fn base64_to_bytes(data: &str) -> Option<Vec<u8>> {
	atob(data).map(|binary| binary.chars().map(|ch| ch as u8).collect())
}

/// Decodes a stored redacted payload. The AWS SDK hands the blob over as bytes, but a
/// persisted session carries it as base64. A hand-edited or externally produced session
/// can hold a signature that is not base64; drop that block instead of failing the
/// whole request.
fn decode_redacted_content(signature: Option<&str>) -> Option<Vec<u8>> {
	base64_to_bytes(signature.filter(|signature| !signature.is_empty())?)
}

/// The `bedrock-converse-stream` API module.
pub struct BedrockConverseStreamStreams;

impl ProviderStreams for BedrockConverseStreamStreams {
	fn stream(
		&self,
		model: &Model,
		context: &TranscriptContext,
		options: Option<ProviderStreamOptions>,
	) -> PiResult<AssistantMessageEventStream> {
		Ok(stream(model, context, options.map(BedrockOptions::from)))
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
