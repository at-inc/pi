//! Port of `packages/ai/src/api/google-vertex.ts`.
//! PORT-STATUS: ported
//!
//! The `@google/genai` SDK is `api/google_client.rs` and Vertex ADC is `api/google_auth.rs`. The TS repeats the
//! google-generative-ai stream loop and `buildParams`; this module calls `stream_google_response` and `build_params`
//! in `google_generative_ai` instead, with its own tool call counter and messages, so the differences listed there
//! (wire-typed chunk reads, the signal passed next to the params) apply here too. Other differences from the TS:
//! - `GoogleVertexOptions` read from `ProviderStreamOptions.extra` take `project` and `location` when they are
//!   strings, besides the `toolChoice` and `thinking` rules of `GoogleOptions`.

use std::cell::RefCell;
use std::ops::{Deref, DerefMut};
use std::rc::Rc;
use std::sync::LazyLock;
use std::sync::atomic::AtomicU64;

use pi_host::{PiError, PiResult};
use regex::Regex;
use serde_json::Value;

use crate::api::google_auth::GoogleAuthOptions;
use crate::api::google_client::{GoogleGenAI, GoogleGenAIOptions, HttpOptions, ResourceScope};
use crate::api::google_generative_ai::{
	GoogleOptions, GoogleThinkingOptions, build_params, stream_google_response, thinking_options_from_json,
	to_provider_headers, tool_choice_name,
};
use crate::api::google_shared::{
	GoogleRetryOptions, ResolvedGoogleThinkingLevel, resolve_google_thinking_level, retry_google_request,
	to_google_thinking_level, uses_google_thinking_level,
};
use crate::api::simple_options::build_base_options;
use crate::models::clamp_thinking_level;
use crate::types::{
	AssistantMessage, AssistantMessageEvent, Model, ModelThinkingLevel, ProviderEnv, ProviderHeaders,
	ProviderStreamOptions, ProviderStreams, SharedAssistantMessage, SimpleStreamOptions, StopReason, StreamOptions,
	ThinkingBudgets, TranscriptContext, Usage, api,
};
use crate::utils::error_body::{format_provider_error, normalize_provider_error};
use crate::utils::event_stream::AssistantMessageEventStream;
use crate::utils::headers::provider_headers_to_record;
use crate::utils::identity::{ClientIdentity, ClientIdentityTarget, get_identity_headers};
use crate::utils::provider_env::get_provider_env_value;
use crate::utils::text::js_trim;
use crate::utils::transcript::collapse_system_messages;

/// `GoogleVertexOptions`.
#[derive(Clone, Default)]
pub struct GoogleVertexOptions {
	pub base: StreamOptions,
	/// `"auto" | "none" | "any"`.
	pub tool_choice: Option<String>,
	pub thinking: Option<GoogleThinkingOptions>,
	pub project: Option<String>,
	pub location: Option<String>,
}

impl Deref for GoogleVertexOptions {
	type Target = StreamOptions;

	fn deref(&self) -> &Self::Target {
		&self.base
	}
}

impl DerefMut for GoogleVertexOptions {
	fn deref_mut(&mut self) -> &mut Self::Target {
		&mut self.base
	}
}

impl From<ProviderStreamOptions> for GoogleVertexOptions {
	/// Reads `toolChoice`, `thinking`, `project` and `location` from the extra keys.
	fn from(options: ProviderStreamOptions) -> Self {
		let extra = options.extra;
		let string = |name: &str| extra.get(name).and_then(Value::as_str).map(str::to_string);
		Self {
			base: options.base,
			tool_choice: string("toolChoice"),
			thinking: extra
				.get("thinking")
				.and_then(Value::as_object)
				.map(thinking_options_from_json),
			project: string("project"),
			location: string("location"),
		}
	}
}

const API_VERSION: &str = "v1";
const GCP_VERTEX_CREDENTIALS_MARKER: &str = "gcp-vertex-credentials";

// Counter for generating unique tool call IDs
static TOOL_CALL_COUNTER: AtomicU64 = AtomicU64::new(0);

/// `stream`: streams one Vertex AI `generateContentStream` response.
pub fn stream(
	model: &Model,
	context: &TranscriptContext,
	options: Option<GoogleVertexOptions>,
) -> AssistantMessageEventStream {
	let stream = AssistantMessageEventStream::new();
	let normalized_context = collapse_system_messages(context);
	let output: SharedAssistantMessage = Rc::new(RefCell::new(AssistantMessage {
		content: Vec::new(),
		api: api::GOOGLE_VERTEX.into(),
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
	options: &GoogleVertexOptions,
	output: &SharedAssistantMessage,
) -> PiResult<()> {
	if options
		.fetch
		.as_ref()
		.is_some_and(|fetch| !pi_host::is_global_fetch(fetch))
	{
		return Err(PiError::new(
			"Custom fetch is not supported by the Google Vertex adapter",
		));
	}
	let api_key = resolve_api_key(options);
	// Create the client using either a Vertex API key, if provided, or ADC with project and location
	let client = match api_key {
		Some(api_key) => {
			create_client_with_api_key(model, api_key, options.headers.as_ref(), options.identity.as_ref())?
		}
		None => create_client(
			model,
			resolve_project(options)?,
			resolve_location(options)?,
			options.headers.as_ref(),
			options.env.as_ref(),
			options.identity.as_ref(),
		)?,
	};
	let mut params = build_params(
		model,
		context,
		&GoogleOptions {
			base: options.base.clone(),
			tool_choice: options.tool_choice.clone(),
			thinking: options.thinking,
		},
	)?;
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
		"Google Vertex stream ended without a finish reason",
	)
	.await
}

/// `streamSimple`: `stream` with the provider-neutral simple options.
pub fn stream_simple(
	model: &Model,
	context: &TranscriptContext,
	options: Option<SimpleStreamOptions>,
) -> PiResult<AssistantMessageEventStream> {
	let base = GoogleVertexOptions {
		base: build_base_options(model, context, options.as_ref(), None),
		tool_choice: options
			.as_ref()
			.and_then(|options| options.tool_choice)
			.map(|tool_choice| tool_choice_name(tool_choice).to_string()),
		..GoogleVertexOptions::default()
	};
	let with_thinking = |thinking: GoogleThinkingOptions| GoogleVertexOptions {
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

/// The google-vertex API module as `ProviderStreams`.
pub struct GoogleVertexStreams;

impl ProviderStreams for GoogleVertexStreams {
	fn stream(
		&self,
		model: &Model,
		context: &TranscriptContext,
		options: Option<ProviderStreamOptions>,
	) -> PiResult<AssistantMessageEventStream> {
		Ok(stream(model, context, options.map(GoogleVertexOptions::from)))
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

fn create_client(
	model: &Model,
	project: String,
	location: String,
	options_headers: Option<&ProviderHeaders>,
	env: Option<&ProviderEnv>,
	identity: Option<&ClientIdentity>,
) -> PiResult<GoogleGenAI> {
	GoogleGenAI::new(GoogleGenAIOptions {
		vertexai: Some(true),
		project: Some(project),
		location: Some(location),
		api_version: Some(API_VERSION.into()),
		google_auth_options: build_google_auth_options(env),
		http_options: build_http_options(model, options_headers, identity)?,
		..GoogleGenAIOptions::default()
	})
}

fn create_client_with_api_key(
	model: &Model,
	api_key: &str,
	options_headers: Option<&ProviderHeaders>,
	identity: Option<&ClientIdentity>,
) -> PiResult<GoogleGenAI> {
	GoogleGenAI::new(GoogleGenAIOptions {
		vertexai: Some(true),
		api_key: Some(api_key.into()),
		api_version: Some(API_VERSION.into()),
		http_options: build_http_options(model, options_headers, identity)?,
		..GoogleGenAIOptions::default()
	})
}

fn build_http_options(
	model: &Model,
	options_headers: Option<&ProviderHeaders>,
	identity: Option<&ClientIdentity>,
) -> PiResult<Option<HttpOptions>> {
	let mut http_options = HttpOptions::default();
	if let Some(base_url) = resolve_custom_base_url(&model.base_url) {
		http_options.base_url_resource_scope = Some(ResourceScope::Collection);
		if base_url_includes_api_version(base_url) {
			http_options.api_version = Some(String::new());
		}
		http_options.base_url = Some(base_url.into());
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
	Ok((http_options != HttpOptions::default()).then_some(http_options))
}

fn resolve_custom_base_url(base_url: &str) -> Option<&str> {
	let trimmed = js_trim(base_url);
	if trimmed.is_empty() || trimmed.contains("{location}") {
		return None;
	}
	Some(trimmed)
}

static API_VERSION_SEGMENT: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^v\d+(?:beta\d*)?$").expect("valid regex"));
static API_VERSION_IN_URL: LazyLock<Regex> =
	LazyLock::new(|| Regex::new(r"(?:^|/)v\d+(?:beta\d*)?(?:/|$)").expect("valid regex"));

fn base_url_includes_api_version(base_url: &str) -> bool {
	match url::Url::parse(base_url) {
		Ok(url) => url.path().split('/').any(|part| API_VERSION_SEGMENT.is_match(part)),
		Err(_) => API_VERSION_IN_URL.is_match(base_url),
	}
}

fn build_google_auth_options(env: Option<&ProviderEnv>) -> Option<GoogleAuthOptions> {
	let key_filename = get_provider_env_value("GOOGLE_APPLICATION_CREDENTIALS", env)?;
	Some(GoogleAuthOptions {
		key_filename: Some(key_filename),
		scopes: None,
	})
}

fn resolve_api_key(options: &GoogleVertexOptions) -> Option<&str> {
	let api_key = js_trim(options.api_key.as_deref()?);
	if api_key.is_empty() || api_key == GCP_VERTEX_CREDENTIALS_MARKER || is_placeholder_api_key(api_key) {
		return None;
	}
	Some(api_key)
}

static PLACEHOLDER_API_KEY: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^<[^>]+>$").expect("valid regex"));

fn is_placeholder_api_key(api_key: &str) -> bool {
	PLACEHOLDER_API_KEY.is_match(api_key)
}

fn resolve_project(options: &GoogleVertexOptions) -> PiResult<String> {
	options
		.project
		.clone()
		.filter(|project| !project.is_empty())
		.or_else(|| get_provider_env_value("GOOGLE_CLOUD_PROJECT", options.env.as_ref()))
		.or_else(|| get_provider_env_value("GCLOUD_PROJECT", options.env.as_ref()))
		.ok_or_else(|| {
			PiError::new(
				"Vertex AI requires a project ID. Set GOOGLE_CLOUD_PROJECT/GCLOUD_PROJECT or pass project in options.",
			)
		})
}

fn resolve_location(options: &GoogleVertexOptions) -> PiResult<String> {
	options
		.location
		.clone()
		.filter(|location| !location.is_empty())
		.or_else(|| get_provider_env_value("GOOGLE_CLOUD_LOCATION", options.env.as_ref()))
		.ok_or_else(|| {
			PiError::new("Vertex AI requires a location. Set GOOGLE_CLOUD_LOCATION or pass location in options.")
		})
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
