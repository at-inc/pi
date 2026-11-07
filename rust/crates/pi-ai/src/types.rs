//! Port of `packages/ai/src/types.ts`.
//! PORT-STATUS: reviewed
//!
//! Dropped type-level helpers with no runtime effect: `IsAny`, `IsExactlyJsonValue`, `IsJsonProperty`,
//! `InvalidJsonKeys`, `IsJsonCompatible`, `JsonRepresentation`, `ApiOptionsMap`, `ApiStreamOptions`,
//! `ModelTypeMap`. Generic types whose parameter only narrows a field (`Model<TApi>`, `ToolResultMessage<TDetails>`,
//! `Tool<TParameters>`) are one type with the widest field type; `Model.compat` is the raw JSON object with typed
//! accessors. Serialized structs list their fields in the TS interface order, so `JSON.stringify` output of a value
//! whose TS object literal used another order differs in key order only. Message, block, usage, tool and model
//! structs keep the app fields of the TS object in a flattened `extra`, written after the declared fields.

use std::cell::RefCell;
use std::fmt;
use std::ops::{Deref, DerefMut};
use std::rc::Rc;

use indexmap::IndexMap;
use pi_host::{AbortSignal, LocalBoxFuture, PiResult};
use pi_telemetry::TelemetrySpan;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::{Map, Value};

use crate::utils::diagnostics::AssistantMessageDiagnostic;
use crate::utils::event_stream::AssistantMessageEventStream;
use crate::utils::identity::ClientIdentity;

pub use crate::providers::compat_schema::{
	AnthropicAllowedFallbackModel, AnthropicMessagesCompat, BedrockCompat, ChatTemplateKwargValue,
	MistralConversationsCompat, OpenAICompletionsCompat, OpenAIResponsesCompat, OpenRouterRouting,
	SessionAffinityFormat, SystemOneCompat, ThinkingTokenBudgetField, VercelGatewayRouting,
};
pub use crate::providers::model_schema::{
	CacheRetention, ModelCost, ModelCostRates, ModelCostTier, ModelImageInputLimits, ModelImageResizeOptions,
	ModelInputLimits, ModelInputModality, ModelPromptCache, ModelThinkingLevel, ThinkingLevel, ThinkingLevelMap,
};
pub use pi_host::FetchFunction;

macro_rules! open_string {
	($(#[$doc:meta])* $name:ident) => {
		$(#[$doc])*
		#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
		#[serde(transparent)]
		pub struct $name(pub String);

		impl From<&str> for $name {
			fn from(value: &str) -> Self {
				Self(value.to_string())
			}
		}

		impl From<String> for $name {
			fn from(value: String) -> Self {
				Self(value)
			}
		}

		impl Deref for $name {
			type Target = str;

			fn deref(&self) -> &str {
				&self.0
			}
		}

		impl fmt::Display for $name {
			fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
				f.write_str(&self.0)
			}
		}

		impl PartialEq<str> for $name {
			fn eq(&self, other: &str) -> bool {
				self.0 == other
			}
		}

		impl PartialEq<&str> for $name {
			fn eq(&self, other: &&str) -> bool {
				self.0 == *other
			}
		}
	};
}

/// Serializes a block or message enum through its variant's struct, which writes its own `type`/`role` tag. A
/// derived internally tagged `Serialize` would write the tag twice.
macro_rules! serialize_by_variant {
	($name:ident { $($variant:ident),* $(,)? }) => {
		impl Serialize for $name {
			fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
				match self {
					$(Self::$variant(value) => value.serialize(serializer),)*
				}
			}
		}
	};
}

open_string!(
	/// `Api = KnownApi | (string & {})`. Known values are in `api`.
	Api
);

/// Known `Api` values (`KnownApi`).
pub mod api {
	pub const OPENAI_COMPLETIONS: &str = "openai-completions";
	pub const MISTRAL_CONVERSATIONS: &str = "mistral-conversations";
	pub const OPENAI_RESPONSES: &str = "openai-responses";
	pub const AZURE_OPENAI_RESPONSES: &str = "azure-openai-responses";
	pub const OPENAI_CODEX_RESPONSES: &str = "openai-codex-responses";
	pub const ANTHROPIC_MESSAGES: &str = "anthropic-messages";
	pub const BEDROCK_CONVERSE_STREAM: &str = "bedrock-converse-stream";
	pub const GOOGLE_GENERATIVE_AI: &str = "google-generative-ai";
	pub const GOOGLE_VERTEX: &str = "google-vertex";
	pub const PI_MESSAGES: &str = "pi-messages";
}

open_string!(
	/// `ImageApi = KnownImageApi | (string & {})`. Known values are in `image_api`.
	ImageApi
);

/// Known `ImageApi` values (`KnownImageApi`).
pub mod image_api {
	pub const OPENAI_CODEX_IMAGES: &str = "openai-codex-images";
	pub const OPENROUTER_IMAGES: &str = "openrouter-images";
}

open_string!(
	/// `ClassifierApi = KnownClassifierApi | (string & {})`. Known values are in `classifier_api`.
	ClassifierApi
);

/// Known `ClassifierApi` values (`KnownClassifierApi`).
pub mod classifier_api {
	pub const TYPESAFE_SYSTEM_ONE: &str = "typesafe-system-one";
	pub const CLOUDFLARE_WORKERS_AI_SYSTEM_ONE: &str = "cloudflare-workers-ai-system-one";
	pub const LLAMA_CPP_CLASSIFY: &str = "llama-cpp-classify";
	pub const OPENAI_DECISIONS: &str = "openai-decisions";
}

open_string!(
	/// `ProviderId = KnownProvider | string`. Known values are in `provider_id`.
	ProviderId
);

/// Known `ProviderId` values (`KnownProvider`).
pub mod provider_id {
	pub const AMAZON_BEDROCK: &str = "amazon-bedrock";
	pub const ANT_LING: &str = "ant-ling";
	pub const ANTHROPIC: &str = "anthropic";
	pub const GOOGLE: &str = "google";
	pub const GOOGLE_VERTEX: &str = "google-vertex";
	pub const OPENAI: &str = "openai";
	pub const AZURE: &str = "azure";
	pub const OPENAI_CODEX: &str = "openai-codex";
	pub const RADIUS: &str = "radius";
	pub const TYPESAFE: &str = "typesafe";
	pub const NVIDIA: &str = "nvidia";
	pub const DEEPSEEK: &str = "deepseek";
	pub const GITHUB_COPILOT: &str = "github-copilot";
	pub const XAI: &str = "xai";
	pub const GROQ: &str = "groq";
	pub const CEREBRAS: &str = "cerebras";
	pub const OPENROUTER: &str = "openrouter";
	pub const VERCEL_AI_GATEWAY: &str = "vercel-ai-gateway";
	pub const ZAI: &str = "zai";
	pub const ZAI_CODING_CN: &str = "zai-coding-cn";
	pub const MISTRAL: &str = "mistral";
	pub const MINIMAX: &str = "minimax";
	pub const MINIMAX_CN: &str = "minimax-cn";
	pub const MOONSHOTAI: &str = "moonshotai";
	pub const MOONSHOTAI_CN: &str = "moonshotai-cn";
	pub const HUGGINGFACE: &str = "huggingface";
	pub const FIREWORKS: &str = "fireworks";
	pub const TOGETHER: &str = "together";
	pub const BASETEN: &str = "baseten";
	pub const OPENCODE: &str = "opencode";
	pub const OPENCODE_GO: &str = "opencode-go";
	pub const KIMI_CODING: &str = "kimi-coding";
	pub const META: &str = "meta";
	pub const CLOUDFLARE_WORKERS_AI: &str = "cloudflare-workers-ai";
	pub const CLOUDFLARE_AI_GATEWAY: &str = "cloudflare-ai-gateway";
	pub const QWEN_TOKEN_PLAN: &str = "qwen-token-plan";
	pub const QWEN_TOKEN_PLAN_CN: &str = "qwen-token-plan-cn";
	pub const QWEN_TOKEN_PLAN_INDIVIDUAL: &str = "qwen-token-plan-individual";
	pub const XIAOMI: &str = "xiaomi";
	pub const XIAOMI_TOKEN_PLAN_CN: &str = "xiaomi-token-plan-cn";
	pub const XIAOMI_TOKEN_PLAN_AMS: &str = "xiaomi-token-plan-ams";
	pub const XIAOMI_TOKEN_PLAN_SGP: &str = "xiaomi-token-plan-sgp";
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ToolChoice {
	#[serde(rename = "auto")]
	Auto,
	#[serde(rename = "none")]
	None,
}

pub type SamplingParams = Map<String, Value>;
pub type SamplingParamsByThinkingLevel = IndexMap<ModelThinkingLevel, SamplingParams>;

/// Token budgets for each thinking level (token-based providers only)
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThinkingBudgets {
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub minimal: Option<f64>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub low: Option<f64>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub medium: Option<f64>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub high: Option<f64>,
}

// Base options all providers share
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Transport {
	#[serde(rename = "sse")]
	Sse,
	#[serde(rename = "websocket")]
	Websocket,
	#[serde(rename = "websocket-cached")]
	WebsocketCached,
	#[serde(rename = "auto")]
	Auto,
}

/// Provider-scoped environment overrides. Values take precedence over process.env.
pub type ProviderEnv = IndexMap<String, String>;
/// Request headers; a `None` value (TS `null`) suppresses a default header with the same name.
pub type ProviderHeaders = IndexMap<String, Option<String>>;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct ProviderResponse {
	pub status: u16,
	pub headers: IndexMap<String, String>,
}

/// `onPayload`: inspects or replaces a provider payload before sending. `Ok(None)` keeps the payload unchanged.
pub type OnPayload<M = Model> = Rc<dyn Fn(Value, &M) -> LocalBoxFuture<'static, PiResult<Option<Value>>>>;
/// `onResponse`: invoked after an HTTP response is received.
pub type OnResponse<M = Model> = Rc<dyn Fn(ProviderResponse, &M) -> LocalBoxFuture<'static, PiResult<()>>>;
/// `onProviderStreamEvent`: observes each parsed provider stream event before Pi normalization.
pub type OnProviderStreamEvent = Rc<dyn Fn(&Value, &Model) -> LocalBoxFuture<'static, PiResult<()>>>;

/// Authentication, HTTP transport, and lifecycle callbacks shared by provider requests.
pub struct ProviderRequestOptions<M = Model> {
	pub signal: Option<AbortSignal>,
	/// Explicit parent span for telemetry produced by this logical request.
	pub telemetry: Option<Rc<dyn TelemetrySpan>>,
	pub api_key: Option<String>,
	/// Optional fetch implementation for provider HTTP requests.
	/// Defaults to `globalThis.fetch`. Provider adapters that cannot inject a custom implementation may reject it.
	/// This does not affect WebSocket transports.
	pub fetch: Option<FetchFunction>,
	/// Provider-scoped environment values. These take precedence over process.env for
	/// provider configuration such as regional settings, endpoint placeholders, and
	/// proxy variables.
	pub env: Option<ProviderEnv>,
	/// Optional callback for inspecting or replacing provider payloads before sending.
	/// Return undefined to keep the payload unchanged.
	pub on_payload: Option<OnPayload<M>>,
	/// Optional callback invoked after an HTTP response is received.
	pub on_response: Option<OnResponse<M>>,
	/// Optional custom HTTP headers to include in API requests.
	/// Merged with provider defaults; caller values override default headers.
	/// On AWS Bedrock these are injected via a Smithy `build`-step middleware so
	/// they are covered by SigV4 signing; reserved headers (`x-amz-*`,
	/// `authorization`, `host`) are silently ignored to preserve SigV4 / bearer auth.
	/// A null value suppresses a provider/API default header with the same name.
	pub headers: Option<ProviderHeaders>,
	/// How the request identifies the calling app: `User-Agent` and provider attribution headers.
	/// Absent fields use pi-ai's defaults: a detailed `User-Agent` and no attribution.
	pub identity: Option<ClientIdentity>,
	/// HTTP request timeout in milliseconds for providers/SDKs that support it.
	/// For example, OpenAI and Anthropic SDK clients default to 10 minutes.
	pub timeout_ms: Option<f64>,
	/// Maximum retry attempts for providers/SDKs that support client-side retries.
	/// For example, OpenAI and Anthropic SDK clients default to 2.
	pub max_retries: Option<f64>,
	/// Maximum delay in milliseconds to wait for a retry when the server requests a long wait.
	/// If the server's requested delay exceeds this value, the request fails immediately
	/// with an error containing the requested delay, allowing higher-level retry logic
	/// to handle it with user visibility.
	/// Default: 60000 (60 seconds). Set to 0 to disable the cap.
	pub max_retry_delay_ms: Option<f64>,
}

impl<M> Clone for ProviderRequestOptions<M> {
	fn clone(&self) -> Self {
		Self {
			signal: self.signal.clone(),
			telemetry: self.telemetry.clone(),
			api_key: self.api_key.clone(),
			fetch: self.fetch.clone(),
			env: self.env.clone(),
			on_payload: self.on_payload.clone(),
			on_response: self.on_response.clone(),
			headers: self.headers.clone(),
			identity: self.identity.clone(),
			timeout_ms: self.timeout_ms,
			max_retries: self.max_retries,
			max_retry_delay_ms: self.max_retry_delay_ms,
		}
	}
}

impl<M> Default for ProviderRequestOptions<M> {
	fn default() -> Self {
		Self {
			signal: None,
			telemetry: None,
			api_key: None,
			fetch: None,
			env: None,
			on_payload: None,
			on_response: None,
			headers: None,
			identity: None,
			timeout_ms: None,
			max_retries: None,
			max_retry_delay_ms: None,
		}
	}
}

/// `StreamOptions`. Its `onResponse` (invoked after an HTTP response is received and before its body stream is
/// consumed) is `base.on_response`.
#[derive(Clone, Default)]
pub struct StreamOptions {
	pub base: ProviderRequestOptions<Model>,
	/// Optional observer for each parsed provider stream event before Pi normalization.
	/// Event data is adapter-owned and must be treated as read-only.
	/// Adapter support is explicit; unsupported adapters do not invoke it.
	pub on_provider_stream_event: Option<OnProviderStreamEvent>,
	pub temperature: Option<f64>,
	/// Arbitrary sampling parameters merged into the request body as-is, after the named request
	/// fields, so keys here override them. Lets custom OpenAI-compatible servers (llama.cpp, vLLM,
	/// SGLang, ...) receive parameters pi does not model, e.g. `top_p`, `top_k`, `min_p`,
	/// `repetition_penalty`. Merged over `Model.samplingParams` per key. Only applied by
	/// OpenAI-compatible adapters (completions, responses, Azure responses); other APIs ignore it.
	pub sampling_params: Option<SamplingParams>,
	pub max_tokens: Option<f64>,
	/// Preferred transport for providers that support multiple transports.
	/// Providers that do not support this option ignore it.
	pub transport: Option<Transport>,
	/// Prompt cache retention preference. Providers map this to their supported values.
	/// Default: "short".
	pub cache_retention: Option<CacheRetention>,
	/// Optional session identifier for providers that support session-based caching.
	/// Providers can use this to enable prompt caching, request routing, or other
	/// session-aware features. Ignored by providers that don't support it.
	pub session_id: Option<String>,
	/// WebSocket connect timeout in milliseconds for providers that support
	/// WebSocket transports. This covers the connection/open handshake only;
	/// stream idleness after connection uses timeoutMs.
	pub websocket_connect_timeout_ms: Option<f64>,
	/// Optional metadata to include in API requests.
	/// Providers extract the fields they understand and ignore the rest.
	/// For example, Anthropic uses `user_id` for abuse tracking and rate limiting.
	pub metadata: Option<Map<String, Value>>,
}

impl Deref for StreamOptions {
	type Target = ProviderRequestOptions<Model>;

	fn deref(&self) -> &Self::Target {
		&self.base
	}
}

impl DerefMut for StreamOptions {
	fn deref_mut(&mut self) -> &mut Self::Target {
		&mut self.base
	}
}

/// `StreamOptions & Record<string, unknown>`: the extra keys an API's own options add.
#[derive(Clone, Default)]
pub struct ProviderStreamOptions {
	pub base: StreamOptions,
	pub extra: Map<String, Value>,
}

impl Deref for ProviderStreamOptions {
	type Target = StreamOptions;

	fn deref(&self) -> &Self::Target {
		&self.base
	}
}

impl DerefMut for ProviderStreamOptions {
	fn deref_mut(&mut self) -> &mut Self::Target {
		&mut self.base
	}
}

#[derive(Clone, Default)]
pub struct DeferredFetchOptions {
	pub base: ProviderRequestOptions<Model>,
	/// Maximum provider long-poll duration in milliseconds.
	/// Defaults to 0, which performs one status check.
	pub wait: Option<f64>,
}

impl Deref for DeferredFetchOptions {
	type Target = ProviderRequestOptions<Model>;

	fn deref(&self) -> &Self::Target {
		&self.base
	}
}

impl DerefMut for DeferredFetchOptions {
	fn deref_mut(&mut self) -> &mut Self::Target {
		&mut self.base
	}
}

/// Request options for best-effort deferred-response cancellation.
pub type DeferredCancelOptions = ProviderRequestOptions<Model>;

/// `ProviderStreams.fetchDeferred`.
pub type FetchDeferredFunction =
	Rc<dyn Fn(&Model, &DeferredHandle, Option<DeferredFetchOptions>) -> PiResult<AssistantMessageEventStream>>;
/// `ProviderStreams.cancelDeferred`.
pub type CancelDeferredFunction =
	Rc<dyn Fn(&Model, &DeferredHandle, Option<DeferredCancelOptions>) -> LocalBoxFuture<'static, PiResult<()>>>;

/// The uniform stream contract of an API implementation module: every module
/// under `src/api/` exports `stream` and `streamSimple`; capable modules may also
/// export deferred-response methods. Lazy wrappers (`lazyApi()`) and provider
/// factories pass these around as values. This is the untyped dispatch shape;
/// per-API option typing lives on the implementation modules themselves and on
/// `Provider.stream()` via `ApiStreamOptions`.
///
/// `stream` takes `ProviderStreamOptions` so an API's own option keys reach it. `Err` is the synchronous throw
/// of a missing request auth. The optional TS methods are accessors: `None` when the module has no such method.
pub trait ProviderStreams {
	fn stream(
		&self,
		model: &Model,
		context: &TranscriptContext,
		options: Option<ProviderStreamOptions>,
	) -> PiResult<AssistantMessageEventStream>;

	fn stream_simple(
		&self,
		model: &Model,
		context: &TranscriptContext,
		options: Option<SimpleStreamOptions>,
	) -> PiResult<AssistantMessageEventStream>;

	fn fetch_deferred(&self) -> Option<FetchDeferredFunction> {
		None
	}

	fn cancel_deferred(&self) -> Option<CancelDeferredFunction> {
		None
	}
}

/// The uniform contract of an image-generation API implementation module:
/// every image API module under `src/api/` exports exactly `generateImages`,
/// so the module itself satisfies this interface. Lazy wrappers and
/// `createProvider({ images })` pass these around as values.
pub trait ProviderImages {
	fn generate_images(
		&self,
		model: &ImageModel,
		context: &ImagesContext,
		options: Option<ProviderImagesOptions>,
	) -> LocalBoxFuture<'static, PiResult<AssistantImages>>;
}

/// The uniform contract implemented by classifier API modules.
pub trait ProviderClassifier {
	fn classify(
		&self,
		model: &ClassifierModel,
		context: &ClassifierContext,
		options: Option<ClassifierOptions>,
	) -> LocalBoxFuture<'static, PiResult<ClassifierResult>>;
}

#[derive(Clone, Default)]
pub struct ClassifierOptions {
	pub base: ProviderRequestOptions<ClassifierModel>,
	/// Divides the answer logits by this value before they are normalized into probabilities.
	/// Values above 1 soften the distribution; values below 1 sharpen it. Must be positive.
	/// APIs that cannot apply it ignore it.
	pub temperature: Option<f64>,
}

impl Deref for ClassifierOptions {
	type Target = ProviderRequestOptions<ClassifierModel>;

	fn deref(&self) -> &Self::Target {
		&self.base
	}
}

impl DerefMut for ClassifierOptions {
	fn deref_mut(&mut self) -> &mut Self::Target {
		&mut self.base
	}
}

#[derive(Clone, Default)]
pub struct ImagesOptions {
	pub base: ProviderRequestOptions<ImageModel>,
	/// Optional metadata to include in API requests.
	/// Providers extract the fields they understand and ignore the rest.
	pub metadata: Option<Map<String, Value>>,
}

impl Deref for ImagesOptions {
	type Target = ProviderRequestOptions<ImageModel>;

	fn deref(&self) -> &Self::Target {
		&self.base
	}
}

impl DerefMut for ImagesOptions {
	fn deref_mut(&mut self) -> &mut Self::Target {
		&mut self.base
	}
}

/// `ImagesOptions & Record<string, unknown>`.
#[derive(Clone, Default)]
pub struct ProviderImagesOptions {
	pub base: ImagesOptions,
	pub extra: Map<String, Value>,
}

impl Deref for ProviderImagesOptions {
	type Target = ImagesOptions;

	fn deref(&self) -> &Self::Target {
		&self.base
	}
}

impl DerefMut for ProviderImagesOptions {
	fn deref_mut(&mut self) -> &mut Self::Target {
		&mut self.base
	}
}

// Unified options with reasoning passed to streamSimple() and completeSimple()
#[derive(Clone, Default)]
pub struct SimpleStreamOptions {
	pub base: StreamOptions,
	/// Provider-neutral tool selection for simple requests. When omitted, adapters use provider-specific behavior.
	pub tool_choice: Option<ToolChoice>,
	pub reasoning: Option<ThinkingLevel>,
	/// Ask a capable provider to return a durable handle and continue the request asynchronously.
	pub deferred: Option<DeferredOption>,
	/// Custom token budgets for thinking levels (token-based providers only)
	pub thinking_budgets: Option<ThinkingBudgets>,
	/// Keys beyond `SimpleStreamOptions`. TS callers pass an API's own options (pi-messages' `debug`) through
	/// `streamSimple`, and the adapters that spread the options read them.
	pub extra: Map<String, Value>,
}

impl Deref for SimpleStreamOptions {
	type Target = StreamOptions;

	fn deref(&self) -> &Self::Target {
		&self.base
	}
}

impl DerefMut for SimpleStreamOptions {
	fn deref_mut(&mut self) -> &mut Self::Target {
		&mut self.base
	}
}

/// `SimpleStreamOptions.deferred`: `boolean | { window?: "15m" | "1h" | "24h" }`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum DeferredOption {
	Enabled(bool),
	Window {
		#[serde(default, skip_serializing_if = "Option::is_none")]
		window: Option<DeferredWindow>,
	},
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum DeferredWindow {
	#[serde(rename = "15m")]
	Minutes15,
	#[serde(rename = "1h")]
	Hours1,
	#[serde(rename = "24h")]
	Hours24,
}

// Generic StreamFunction with typed options.
//
// Contract:
// - Receives a normalized transcript: the system prompt and tools live in the
//   leading system message, never on the context itself.
// - Must return an AssistantMessageEventStream.
// - Direct streamSimple() calls may throw synchronously when request auth is
//   missing. Once a stream is returned, request/model/runtime failures should
//   be encoded in that stream.
// - Error termination must produce an AssistantMessage with stopReason
//   "error" or "aborted" and errorMessage, emitted via the stream protocol.
pub type StreamFunction<O = StreamOptions> =
	Rc<dyn Fn(&Model, &TranscriptContext, Option<O>) -> PiResult<AssistantMessageEventStream>>;

pub type ImagesFunction<O = ImagesOptions> =
	Rc<dyn Fn(&ImageModel, &ImagesContext, Option<O>) -> LocalBoxFuture<'static, PiResult<AssistantImages>>>;

pub type ClassifierFunction<O = ClassifierOptions> =
	Rc<dyn Fn(&ClassifierModel, &ClassifierContext, Option<O>) -> LocalBoxFuture<'static, PiResult<ClassifierResult>>>;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TextSignatureV1 {
	pub v: f64,
	pub id: String,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub phase: Option<TextSignaturePhase>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum TextSignaturePhase {
	#[serde(rename = "commentary")]
	Commentary,
	#[serde(rename = "final_answer")]
	FinalAnswer,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename = "text", rename_all = "camelCase")]
pub struct TextContent {
	pub text: String,
	/// e.g., for OpenAI responses, message metadata (legacy id string or TextSignatureV1 JSON)
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub text_signature: Option<String>,
	/// App fields the TS object carried beyond this interface; kept so they survive a round trip.
	#[serde(flatten, deserialize_with = "extra_without_type")]
	pub extra: JsonObject,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename = "thinking", rename_all = "camelCase")]
pub struct ThinkingContent {
	pub thinking: String,
	/// Provider-specific opaque or serialized reasoning replay data
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub thinking_signature: Option<String>,
	/// When true, the thinking content was redacted by safety filters. The opaque
	/// encrypted payload is stored in `thinkingSignature` so it can be passed back
	/// to the API for multi-turn continuity.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub redacted: Option<bool>,
	/// App fields the TS object carried beyond this interface; kept so they survive a round trip.
	#[serde(flatten, deserialize_with = "extra_without_type")]
	pub extra: JsonObject,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename = "image", rename_all = "camelCase")]
pub struct ImageContent {
	/// base64 encoded image data
	pub data: String,
	/// e.g., "image/jpeg", "image/png"
	pub mime_type: String,
	/// App fields the TS object carried beyond this interface; kept so they survive a round trip.
	#[serde(flatten, deserialize_with = "extra_without_type")]
	pub extra: JsonObject,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename = "toolCall", rename_all = "camelCase")]
pub struct ToolCall {
	pub id: String,
	pub name: String,
	#[serde(default)]
	pub arguments: JsonObject,
	/// Google-specific: opaque signature for reusing thought context
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub thought_signature: Option<String>,
	/// OpenAI Responses namespace for calls to dynamically loaded or namespaced tools.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub namespace: Option<String>,
	/// Set when the model's complete arguments text was not a valid JSON object. `arguments` is then `{}`.
	/// Describes the problem and includes the text the model sent; tool argument validation fails with it.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub arguments_error: Option<String>,
	/// App fields the TS object carried beyond this interface; kept so they survive a round trip.
	#[serde(flatten, deserialize_with = "extra_without_type")]
	pub extra: JsonObject,
}

/// A block of `AssistantMessage.content`: `TextContent | ThinkingContent | ToolCall`.
#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(tag = "type")]
pub enum AssistantContent {
	#[serde(rename = "text")]
	Text(TextContent),
	#[serde(rename = "thinking")]
	Thinking(ThinkingContent),
	#[serde(rename = "toolCall")]
	ToolCall(ToolCall),
}

serialize_by_variant!(AssistantContent {
	Text,
	Thinking,
	ToolCall
});

/// A block of user and tool-result content: `TextContent | ImageContent`.
#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(tag = "type")]
pub enum UserContent {
	#[serde(rename = "text")]
	Text(TextContent),
	#[serde(rename = "image")]
	Image(ImageContent),
}

serialize_by_variant!(UserContent { Text, Image });

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Usage {
	pub input: f64,
	pub output: f64,
	pub cache_read: f64,
	pub cache_write: f64,
	/// Subset of `cacheWrite` written with 1h retention. Only Anthropic reports this split.
	#[serde(rename = "cacheWrite1h", default, skip_serializing_if = "Option::is_none")]
	pub cache_write_1h: Option<f64>,
	/// Reasoning/thinking tokens, when the provider reports them. This is a subset of
	/// `output`: `output` already includes these tokens. Set to a number (possibly 0) by
	/// providers that expose a reasoning breakdown; left undefined by providers that don't.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub reasoning: Option<f64>,
	pub total_tokens: f64,
	pub cost: UsageCost,
	/// App fields the TS object carried beyond this interface; kept so they survive a round trip.
	#[serde(flatten)]
	pub extra: JsonObject,
}

/// `Usage.cost`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageCost {
	/// Catalog-derived input cost.
	pub input: f64,
	/// Catalog-derived output cost.
	pub output: f64,
	/// Catalog-derived cache-read cost.
	pub cache_read: f64,
	/// Catalog-derived cache-write cost.
	pub cache_write: f64,
	/// Best available total: provider-reported billed cost when supported, otherwise the catalog estimate.
	pub total: f64,
	/// True when `total` is the billed cost reported by the provider. False when it is a catalog estimate; absent also means false.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub total_from_provider: Option<bool>,
	/// App fields the TS object carried beyond this interface; kept so they survive a round trip.
	#[serde(flatten)]
	pub extra: JsonObject,
}

/// `Default` (`Stop`) exists only so message literals can use `..Default::default()`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum StopReason {
	#[serde(rename = "pending")]
	Pending,
	#[default]
	#[serde(rename = "stop")]
	Stop,
	#[serde(rename = "length")]
	Length,
	#[serde(rename = "toolUse")]
	ToolUse,
	#[serde(rename = "error")]
	Error,
	#[serde(rename = "aborted")]
	Aborted,
	#[serde(rename = "deferred")]
	Deferred,
}

pub type JsonValue = Value;
pub type JsonObject = Map<String, Value>;

/// Deserializes an optional `JsonValue` field so that an explicit `null` stays `Some(Value::Null)` instead of becoming
/// absent. Use with `#[serde(default, skip_serializing_if = "Option::is_none")]`.
fn present_json_value<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Option<Value>, D::Error> {
	Value::deserialize(deserializer).map(Some)
}

/// A missing or `null` message `content` reads as an empty list, as `transformMessages` normalizes `content == null`
/// from untyped callers and old session files (#6259, #6276).
fn null_as_empty<'de, D: Deserializer<'de>, T: Deserialize<'de>>(deserializer: D) -> Result<Vec<T>, D::Error> {
	Ok(Option::<Vec<T>>::deserialize(deserializer)?.unwrap_or_default())
}

/// `null_as_empty` for `UserMessage.content`, which TS normalizes to `[]` rather than `""`.
fn user_content_null_as_empty<'de, D: Deserializer<'de>>(deserializer: D) -> Result<UserMessageContent, D::Error> {
	Ok(Option::<UserMessageContent>::deserialize(deserializer)?.unwrap_or_else(empty_user_content))
}

fn empty_user_content() -> UserMessageContent {
	UserMessageContent::Blocks(Vec::new())
}

/// The `extra` of a block deserialized on its own, where serde hands the `type` tag to the flattened map too. The
/// struct writes its tag itself, so keeping it here would write it twice.
fn extra_without_type<'de, D: Deserializer<'de>>(deserializer: D) -> Result<JsonObject, D::Error> {
	let mut extra = JsonObject::deserialize(deserializer)?;
	extra.shift_remove("type");
	Ok(extra)
}

/// `extra_without_type` for the `role` tag of a message.
fn extra_without_role<'de, D: Deserializer<'de>>(deserializer: D) -> Result<JsonObject, D::Error> {
	let mut extra = JsonObject::deserialize(deserializer)?;
	extra.shift_remove("role");
	Ok(extra)
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeferredHandle {
	pub provider: String,
	pub model_id: String,
	pub api: String,
	/// Provider token, such as a response id or batch id plus row id.
	pub id: String,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub expires_at: Option<f64>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub poll_after_ms: Option<f64>,
	/// Provider conversion data required to reconstruct the final assistant message.
	#[serde(
		default,
		deserialize_with = "present_json_value",
		skip_serializing_if = "Option::is_none"
	)]
	pub data: Option<JsonValue>,
	/// App fields the TS object carried beyond this interface; kept so they survive a round trip.
	#[serde(flatten)]
	pub extra: JsonObject,
}

/// System instructions and tool declarations at one point in the transcript.
///
/// The leading system message is the system prompt. Later system messages change it:
/// `content` adds instructions from that point on, `sections` replace or remove named
/// prompt sections, and `toolsAdded`/`toolsRemoved` change the tool set. Replaying
/// every system message in order yields the current prompt and tools. Providers that
/// accept system messages mid-conversation send each one in place; other providers
/// rebuild the leading system message from the replayed state.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(tag = "role", rename = "system", rename_all = "camelCase")]
pub struct SystemMessage {
	/// Instruction text. On the leading message this is the base prompt; later, additional instructions.
	pub content: SystemMessageContent,
	/// Named, ordered prompt sections rendered verbatim after `content`. The leading message
	/// declares them; later messages replace sections by name, and `null` removes one. Keep
	/// each section self-delimiting (a tag, a heading) so the model can relate an update to
	/// the original. Avoid integer-like names; JSON objects reorder those.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub sections: Option<IndexMap<String, Option<String>>>,
	/// Complete definitions of tools that become available at this point.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub tools_added: Option<Vec<Tool>>,
	/// Tools that stop being available at this point.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub tools_removed: Option<Vec<ToolReference>>,
	/// Unix timestamp in milliseconds
	pub timestamp: f64,
	/// App fields the TS object carried beyond this interface; kept so they survive a round trip.
	#[serde(flatten, deserialize_with = "extra_without_role")]
	pub extra: JsonObject,
}

/// `SystemMessage.content`: `string | TextContent[]`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum SystemMessageContent {
	Text(String),
	Blocks(Vec<TextContent>),
}

impl Default for SystemMessageContent {
	fn default() -> Self {
		Self::Text(String::new())
	}
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(tag = "role", rename = "user", rename_all = "camelCase")]
pub struct UserMessage {
	#[serde(default = "empty_user_content", deserialize_with = "user_content_null_as_empty")]
	pub content: UserMessageContent,
	/// Unix timestamp in milliseconds
	pub timestamp: f64,
	/// App fields the TS object carried beyond this interface; kept so they survive a round trip.
	#[serde(flatten, deserialize_with = "extra_without_role")]
	pub extra: JsonObject,
}

/// `UserMessage.content`: `string | (TextContent | ImageContent)[]`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum UserMessageContent {
	Text(String),
	Blocks(Vec<UserContent>),
}

impl Default for UserMessageContent {
	fn default() -> Self {
		Self::Text(String::new())
	}
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(tag = "role", rename = "assistant", rename_all = "camelCase")]
pub struct AssistantMessage {
	#[serde(default, deserialize_with = "null_as_empty")]
	pub content: Vec<AssistantContent>,
	pub api: Api,
	pub provider: ProviderId,
	pub model: String,
	/// Concrete model reported by the provider, when reported. May equal the requested `model`
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub response_model: Option<String>,
	/// Provider-specific response/message identifier when the upstream API exposes one
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub response_id: Option<String>,
	/// Exact provider-native effort level used for this response. Absent for legacy or unmanaged responses.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub provider_thinking_level: Option<String>,
	/// Pi thinking level the agent loop requested for this response. Absent outside the agent loop and for legacy responses.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub thinking_level: Option<ModelThinkingLevel>,
	/// Redacted provider/runtime diagnostics for failures and recoveries.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub diagnostics: Option<Vec<AssistantMessageDiagnostic>>,
	pub usage: Usage,
	pub stop_reason: StopReason,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub deferred: Option<DeferredHandle>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub error_message: Option<String>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub raw_stop_reason: Option<String>,
	/// Provider indication of whether the model explicitly ended its turn.
	/// Preserved for debugging and does not currently affect agent control flow.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub end_turn: Option<bool>,
	/// Unix timestamp in milliseconds when the request started.
	pub timestamp: f64,
	/// Milliseconds from `timestamp` until the response ended, measured with a monotonic clock. Set by
	/// `AssistantMessageEventStream` on the final message of a response it saw start; absent for legacy messages and
	/// for deferred results fetched later.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub duration_ms: Option<f64>,
	/// App fields the TS object carried beyond this interface; kept so they survive a round trip.
	#[serde(flatten, deserialize_with = "extra_without_role")]
	pub extra: JsonObject,
}

/// The live response-so-far that a provider mutates and every event's `partial` shares.
pub type SharedAssistantMessage = Rc<RefCell<AssistantMessage>>;

/// A tool call that another tool made while it ran, for example from a codemode script.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NestedToolCallRecord {
	pub id: String,
	pub name: String,
	/// Omitted when over the size limits; `argumentsBytes` then gives their size.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub arguments: Option<JsonObject>,
	/// UTF-8 size of the arguments as JSON, set when `arguments` is omitted.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub arguments_bytes: Option<f64>,
	/// `unfinished`: the call was still running when the calling tool finished.
	pub status: NestedToolCallStatus,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub duration_ms: Option<f64>,
	/// Error text, truncated.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub error: Option<String>,
}

/// `NestedToolCallRecord.status`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum NestedToolCallStatus {
	#[serde(rename = "ok")]
	Ok,
	#[serde(rename = "error")]
	Error,
	#[serde(rename = "unfinished")]
	Unfinished,
}

/// Bounded record of the nested calls a tool made. Results are not recorded.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NestedToolCalls {
	pub calls: Vec<NestedToolCallRecord>,
	/// False when calls were dropped, arguments omitted, or calls had not finished.
	pub complete: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(tag = "role", rename = "toolResult", rename_all = "camelCase")]
pub struct ToolResultMessage {
	pub tool_call_id: String,
	pub tool_name: String,
	/// Supports text and images
	#[serde(default, deserialize_with = "null_as_empty")]
	pub content: Vec<UserContent>,
	/// An explicit `null` stays `Some(Value::Null)`.
	#[serde(
		default,
		deserialize_with = "present_json_value",
		skip_serializing_if = "Option::is_none"
	)]
	pub details: Option<JsonValue>,
	/// Usage from the tool execution itself, if available. Not part of main LLM context accounting.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub usage: Option<Usage>,
	/// Calls this tool made to other tools. Kept for the session record; not sent to the model.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub nested_calls: Option<NestedToolCalls>,
	pub is_error: bool,
	/// Unix timestamp in milliseconds when the result was created.
	pub timestamp: f64,
	/// Milliseconds the tool's execution took, measured with a monotonic clock. Absent for legacy results.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub duration_ms: Option<f64>,
	/// App fields the TS object carried beyond this interface; kept so they survive a round trip.
	#[serde(flatten, deserialize_with = "extra_without_role")]
	pub extra: JsonObject,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(tag = "role")]
pub enum Message {
	#[serde(rename = "system")]
	System(SystemMessage),
	#[serde(rename = "user")]
	User(UserMessage),
	#[serde(rename = "assistant")]
	Assistant(AssistantMessage),
	#[serde(rename = "toolResult")]
	ToolResult(ToolResultMessage),
}

serialize_by_variant!(Message {
	System,
	User,
	Assistant,
	ToolResult
});

pub type ImagesInputContent = UserContent;
pub type ImagesOutputContent = UserContent;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImagesContext {
	pub input: Vec<ImagesInputContent>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ImagesStopReason {
	#[serde(rename = "stop")]
	Stop,
	#[serde(rename = "error")]
	Error,
	#[serde(rename = "aborted")]
	Aborted,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AssistantImages {
	pub api: ImageApi,
	pub provider: ProviderId,
	pub model: String,
	pub output: Vec<ImagesOutputContent>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub response_id: Option<String>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub usage: Option<Usage>,
	pub stop_reason: ImagesStopReason,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub error_message: Option<String>,
	/// Unix timestamp in milliseconds
	pub timestamp: f64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename = "choice", rename_all = "camelCase")]
pub struct ClassifierChoiceQuestion {
	pub instructions: String,
	pub criteria: IndexMap<String, String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename = "score", rename_all = "camelCase")]
pub struct ClassifierScoreQuestion {
	pub instructions: String,
	pub criteria: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename = "bool", rename_all = "camelCase")]
pub struct ClassifierBoolQuestion {
	pub instructions: String,
	pub criteria: ClassifierBoolCriteria,
}

/// `ClassifierBoolQuestion.criteria`: `{ true: string; false: string }`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ClassifierBoolCriteria {
	pub r#true: String,
	pub r#false: String,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(tag = "type")]
pub enum ClassifierQuestion {
	#[serde(rename = "choice")]
	Choice(ClassifierChoiceQuestion),
	#[serde(rename = "score")]
	Score(ClassifierScoreQuestion),
	#[serde(rename = "bool")]
	Bool(ClassifierBoolQuestion),
}

serialize_by_variant!(ClassifierQuestion { Choice, Score, Bool });

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClassifierContext {
	pub state: JsonObject,
	/// Images judged together with `state`. Only models whose `input` includes `"image"` accept them;
	/// other models return an error result.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub images: Option<Vec<ImageContent>>,
	pub questions: IndexMap<String, ClassifierQuestion>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename = "choice", rename_all = "camelCase")]
pub struct ClassifierChoiceAnswer {
	pub choice: String,
	pub probabilities: IndexMap<String, f64>,
	pub confidence: f64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename = "score", rename_all = "camelCase")]
pub struct ClassifierScoreAnswer {
	pub score: f64,
	pub confidence: f64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename = "bool", rename_all = "camelCase")]
pub struct ClassifierBoolAnswer {
	pub probability: f64,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(tag = "type")]
pub enum ClassifierAnswer {
	#[serde(rename = "choice")]
	Choice(ClassifierChoiceAnswer),
	#[serde(rename = "score")]
	Score(ClassifierScoreAnswer),
	#[serde(rename = "bool")]
	Bool(ClassifierBoolAnswer),
}

serialize_by_variant!(ClassifierAnswer { Choice, Score, Bool });

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ClassifierStopReason {
	#[serde(rename = "stop")]
	Stop,
	#[serde(rename = "error")]
	Error,
	#[serde(rename = "aborted")]
	Aborted,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClassifierResult {
	pub api: ClassifierApi,
	pub provider: ProviderId,
	pub model: String,
	pub answers: IndexMap<String, ClassifierAnswer>,
	/// Token usage and its cost at the model's catalog price, when the service reports token counts.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub usage: Option<Usage>,
	pub stop_reason: ClassifierStopReason,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub error_message: Option<String>,
	/// Unix timestamp in milliseconds
	pub timestamp: f64,
}

/// OpenAI grammar variants for constrained sampling.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum GrammarFormat {
	#[serde(rename = "openai_lark")]
	OpenaiLark,
	#[serde(rename = "openai_regex")]
	OpenaiRegex,
}

pub type GrammarVariants = IndexMap<GrammarFormat, String>;

/// Optional provider-side constrained sampling configs for a tool.
///
/// The `json_schema` value roughly maps to the concept of `strict` in APIs which is
/// implemented as json-schema constrained sampling by APIs. Grammar variants let
/// callers provide provider-specific encodings of the same intended language.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum ConstrainedSamplingConfig {
	#[serde(rename = "json_schema")]
	/// `strict` is optional at runtime: a config without it is neither "prefer" nor "require".
	JsonSchema {
		#[serde(default, skip_serializing_if = "Option::is_none")]
		strict: Option<ConstrainedSamplingStrictness>,
	},
	#[serde(rename = "grammar")]
	Grammar { variants: GrammarVariants },
}

/// `strict` of a `json_schema` constrained sampling config.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ConstrainedSamplingStrictness {
	#[serde(rename = "prefer")]
	Prefer,
	#[serde(rename = "require")]
	Require,
}

/// `Tool.constrainedSampling`: `false | ConstrainedSamplingConfig`. `Disabled(false)` is the `false` literal.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ToolConstrainedSampling {
	Disabled(bool),
	Config(ConstrainedSamplingConfig),
}

/// A tool the model may call. `parameters` is the JSON Schema of its arguments: pi-schema's builders (which also type
/// the arguments, see `Static` in `@earendil-works/pi-schema`), TypeBox schemas, or any JSON Schema object.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Tool {
	pub name: String,
	pub description: String,
	pub parameters: Value,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub constrained_sampling: Option<ToolConstrainedSampling>,
	/// App fields the TS object carried beyond this interface; kept so they survive a round trip.
	#[serde(flatten)]
	pub extra: JsonObject,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolReference {
	pub name: String,
}

/// Request input accepted by the public stream entry points (`Models.stream()`,
/// `streamSimple()`, ...). `systemPrompt` and `tools` are shorthand for a leading
/// system message; `normalizeContext()` folds them into one before the request
/// reaches a provider.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Context {
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub system_prompt: Option<String>,
	pub messages: Vec<Message>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub tools: Option<Vec<Tool>>,
}

/// Normalized request context passed to providers and API implementations. The
/// prompt and tool declarations are carried by the transcript's system messages.
/// Only `normalizeContext()` produces this type, so a raw `Context` cannot reach
/// provider code by accident.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct TranscriptContext {
	pub messages: Vec<Message>,
}

impl TranscriptContext {
	/// Wraps messages that `normalizeContext()` produced. Only `normalize_context` calls this, which keeps the brand.
	pub fn from_normalized_messages(messages: Vec<Message>) -> Self {
		Self { messages }
	}
}

/// Event protocol for AssistantMessageEventStream.
///
/// Successful streams emit `start` before partial updates and terminate with
/// `done`. A stream may terminate directly with `error` when request setup fails
/// before generation starts; after `start`, failures also terminate with `error`.
/// Direct `streamSimple()` calls throw synchronously when request auth is missing.
/// Updates and `done` must never appear before `start`.
///
/// `partial` is the shared live response-so-far helper, not an event-time
/// snapshot. Text and thinking blocks are empty when their `*_start` event is
/// emitted and grow only through their corresponding `*_delta` events until the
/// authoritative `*_end`. Redacted thinking may be complete at start and emit no
/// deltas. Tool-call arguments at `toolcall_start` are provider-specific;
/// `toolcall_delta` carries subsequent JSON updates.
///
/// `Done.reason` is one of `stop`, `length`, `toolUse`, `deferred`; `Error.reason` is `aborted` or `error`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", rename_all_fields = "camelCase")]
pub enum AssistantMessageEvent {
	Start {
		partial: SharedAssistantMessage,
	},
	TextStart {
		content_index: usize,
		partial: SharedAssistantMessage,
	},
	TextDelta {
		content_index: usize,
		delta: String,
		partial: SharedAssistantMessage,
	},
	TextEnd {
		content_index: usize,
		content: String,
		partial: SharedAssistantMessage,
	},
	ThinkingStart {
		content_index: usize,
		partial: SharedAssistantMessage,
	},
	ThinkingDelta {
		content_index: usize,
		delta: String,
		partial: SharedAssistantMessage,
	},
	ThinkingEnd {
		content_index: usize,
		content: String,
		partial: SharedAssistantMessage,
	},
	ToolcallStart {
		content_index: usize,
		partial: SharedAssistantMessage,
	},
	ToolcallDelta {
		content_index: usize,
		delta: String,
		partial: SharedAssistantMessage,
	},
	ToolcallEnd {
		content_index: usize,
		tool_call: ToolCall,
		partial: SharedAssistantMessage,
	},
	Done {
		reason: StopReason,
		message: SharedAssistantMessage,
	},
	Error {
		reason: StopReason,
		error: SharedAssistantMessage,
	},
}

/// Fields shared by every catalog entry, regardless of what you can do with it.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BaseModel<A = Api> {
	pub id: String,
	pub name: String,
	pub api: A,
	pub provider: ProviderId,
	pub base_url: String,
	pub input: Vec<ModelInputModality>,
	/// Provider input limits and cache-safe preprocessing metadata.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub input_limits: Option<ModelInputLimits>,
	pub cost: ModelCost,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub headers: Option<IndexMap<String, String>>,
}

/// Chat model: usable with `stream()` and friends.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Model {
	#[serde(flatten)]
	pub base: BaseModel<Api>,
	/// Optional: chat is the default model type, so models without `type` are chat
	/// models. Narrow mixed model lists with `isModelType()` instead of comparing
	/// `type` directly.
	#[serde(rename = "type", default, skip_serializing_if = "Option::is_none")]
	pub r#type: Option<ModelType>,
	pub reasoning: bool,
	/// Whether this provider/model supports priority or fast inference. Only true declares support; unset means unknown.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub supports_fast_mode: Option<bool>,
	/// Maps pi thinking levels to provider/model-specific values.
	/// Missing keys use provider defaults. null marks a level as unsupported.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub thinking_level_map: Option<ThinkingLevelMap>,
	/// Prompt cache lifetimes per retention tier. Unset when the provider's cache behavior is unknown.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub prompt_cache: Option<ModelPromptCache>,
	pub context_window: f64,
	pub max_tokens: f64,
	/// Default sampling parameters for this model. See {@link StreamOptions.samplingParams}; per-request keys override these.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub sampling_params: Option<SamplingParams>,
	/// Sampling parameter overrides selected by the effective pi thinking level.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub sampling_params_by_thinking_level: Option<SamplingParamsByThinkingLevel>,
	/// Compatibility overrides for OpenAI-compatible APIs. If not set, auto-detected from baseUrl. Read it through
	/// the accessor for the model's API (`openai_completions_compat()`, ...).
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub compat: Option<JsonObject>,
	/// App fields the TS object carried beyond this interface; kept so they survive a round trip.
	#[serde(flatten)]
	pub extra: JsonObject,
}

impl Deref for Model {
	type Target = BaseModel<Api>;

	fn deref(&self) -> &Self::Target {
		&self.base
	}
}

impl DerefMut for Model {
	fn deref_mut(&mut self) -> &mut Self::Target {
		&mut self.base
	}
}

/// `compat` read as `T`, field by field like the TS property reads: a field whose value does not fit `T` reads as
/// absent and leaves the other fields intact.
fn compat_as<T: DeserializeOwned + Default>(compat: Option<&JsonObject>) -> T {
	let Some(compat) = compat else {
		return T::default();
	};
	if let Ok(value) = serde_json::from_value(Value::Object(compat.clone())) {
		return value;
	}
	let fitting: JsonObject = compat
		.iter()
		.filter(|(key, value)| {
			let field = JsonObject::from_iter([((*key).clone(), (*value).clone())]);
			serde_json::from_value::<T>(Value::Object(field)).is_ok()
		})
		.map(|(key, value)| (key.clone(), value.clone()))
		.collect();
	serde_json::from_value(Value::Object(fitting)).unwrap_or_default()
}

impl Model {
	/// `compat` of an `openai-completions` model.
	pub fn openai_completions_compat(&self) -> OpenAICompletionsCompat {
		compat_as(self.compat.as_ref())
	}

	/// `compat` of an `openai-responses`, `azure-openai-responses` or `openai-codex-responses` model.
	pub fn openai_responses_compat(&self) -> OpenAIResponsesCompat {
		compat_as(self.compat.as_ref())
	}

	/// `compat` of an `anthropic-messages` model.
	pub fn anthropic_messages_compat(&self) -> AnthropicMessagesCompat {
		compat_as(self.compat.as_ref())
	}

	/// `compat` of a `bedrock-converse-stream` model.
	pub fn bedrock_compat(&self) -> BedrockCompat {
		compat_as(self.compat.as_ref())
	}

	/// `compat` of a `mistral-conversations` model.
	pub fn mistral_conversations_compat(&self) -> MistralConversationsCompat {
		compat_as(self.compat.as_ref())
	}
}

/// Image-generation model: usable with `generateImages()` only.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImageModel {
	#[serde(flatten)]
	pub base: BaseModel<ImageApi>,
	/// Always `ModelType::Image`.
	#[serde(rename = "type")]
	pub r#type: ModelType,
	/// Output modalities. Always includes `"image"`; `"text"` means the model can also return text blocks.
	pub output: Vec<ModelInputModality>,
	/// App fields the TS object carried beyond this interface; kept so they survive a round trip.
	#[serde(flatten)]
	pub extra: JsonObject,
}

impl Default for ImageModel {
	fn default() -> Self {
		Self {
			base: BaseModel::default(),
			r#type: ModelType::Image,
			output: Vec::new(),
			extra: JsonObject::new(),
		}
	}
}

impl Deref for ImageModel {
	type Target = BaseModel<ImageApi>;

	fn deref(&self) -> &Self::Target {
		&self.base
	}
}

impl DerefMut for ImageModel {
	fn deref_mut(&mut self) -> &mut Self::Target {
		&mut self.base
	}
}

/// Structured classifier model: usable with `classify()` only.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClassifierModel {
	#[serde(flatten)]
	pub base: BaseModel<ClassifierApi>,
	/// Always `ModelType::Classifier`.
	#[serde(rename = "type")]
	pub r#type: ModelType,
	pub context_window: f64,
	/// Compatibility overrides for System One APIs. If not set, auto-detected from provider, baseUrl, and id. Read it
	/// through `system_one_compat()`.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub compat: Option<JsonObject>,
	/// App fields the TS object carried beyond this interface; kept so they survive a round trip.
	#[serde(flatten)]
	pub extra: JsonObject,
}

impl Default for ClassifierModel {
	fn default() -> Self {
		Self {
			base: BaseModel::default(),
			r#type: ModelType::Classifier,
			context_window: 0.0,
			compat: None,
			extra: JsonObject::new(),
		}
	}
}

impl Deref for ClassifierModel {
	type Target = BaseModel<ClassifierApi>;

	fn deref(&self) -> &Self::Target {
		&self.base
	}
}

impl DerefMut for ClassifierModel {
	fn deref_mut(&mut self) -> &mut Self::Target {
		&mut self.base
	}
}

impl ClassifierModel {
	/// `compat` of a `typesafe-system-one` or `cloudflare-workers-ai-system-one` model.
	pub fn system_one_compat(&self) -> SystemOneCompat {
		compat_as(self.compat.as_ref())
	}
}

/// What a catalog entry is for. Decides which `Models` operation accepts it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ModelType {
	#[serde(rename = "chat")]
	Chat,
	#[serde(rename = "image")]
	Image,
	#[serde(rename = "classifier")]
	Classifier,
}

/// Anything a provider can list. Narrow with `isModelType()`. Deserializes by `type`; an absent or `null` `type` is
/// a chat model.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(untagged)]
pub enum AnyModel {
	Chat(Model),
	Image(ImageModel),
	Classifier(ClassifierModel),
	/// An entry whose `type` is none of the known ones, kept as it was read: TS keeps such an object, and
	/// `isModelType()` matches no type for it.
	Other(Value),
}

impl<'de> Deserialize<'de> for AnyModel {
	fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
		let value = Value::deserialize(deserializer)?;
		let model_type = match value.get("type") {
			None | Some(Value::Null) => ModelType::Chat,
			Some(model_type) => match ModelType::deserialize(model_type) {
				Ok(model_type) => model_type,
				Err(_) => return Ok(AnyModel::Other(value)),
			},
		};
		let model = match model_type {
			ModelType::Chat => serde_json::from_value(value).map(AnyModel::Chat),
			ModelType::Image => serde_json::from_value(value).map(AnyModel::Image),
			ModelType::Classifier => serde_json::from_value(value).map(AnyModel::Classifier),
		};
		model.map_err(serde::de::Error::custom)
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use pi_host::js::json_stringify;
	use serde_json::json;

	#[test]
	fn messages_and_blocks_write_their_tag_once_and_read_it_back() {
		let message = Message::User(UserMessage {
			content: UserMessageContent::Blocks(vec![
				UserContent::Text(TextContent {
					text: "hi".into(),
					..TextContent::default()
				}),
				UserContent::Image(ImageContent {
					data: "AA==".into(),
					mime_type: "image/png".into(),
					..ImageContent::default()
				}),
			]),
			timestamp: 1.0,
			..UserMessage::default()
		});
		let serialized = json_stringify(&message);
		assert_eq!(
			serialized,
			r#"{"role":"user","content":[{"type":"text","text":"hi"},{"type":"image","data":"AA==","mimeType":"image/png"}],"timestamp":1}"#
		);
		assert_eq!(serde_json::from_str::<Message>(&serialized).unwrap(), message);

		let call = ToolCall {
			id: "1".into(),
			name: "read".into(),
			arguments: json!({ "path": "a" }).as_object().unwrap().clone(),
			..ToolCall::default()
		};
		assert_eq!(
			json_stringify(&call),
			r#"{"type":"toolCall","id":"1","name":"read","arguments":{"path":"a"}}"#
		);
		let partial = Rc::new(RefCell::new(AssistantMessage {
			content: vec![AssistantContent::ToolCall(call.clone())],
			api: api::OPENAI_RESPONSES.into(),
			provider: provider_id::OPENAI.into(),
			model: "m".into(),
			stop_reason: StopReason::ToolUse,
			timestamp: 2.0,
			..AssistantMessage::default()
		}));
		let event = AssistantMessageEvent::ToolcallEnd {
			content_index: 0,
			tool_call: call,
			partial,
		};
		let serialized = json_stringify(&event);
		assert!(serialized.starts_with(r#"{"type":"toolcall_end","contentIndex":0,"toolCall":{"type":"toolCall","#));
		assert!(serialized.contains(r#""partial":{"role":"assistant","content":[{"type":"toolCall","#));
		assert_eq!(
			serde_json::from_str::<AssistantMessageEvent>(&serialized).unwrap(),
			event
		);
	}

	#[test]
	fn app_fields_survive_a_round_trip() {
		let message = json!({
			"role": "assistant",
			"content": [
				{ "type": "text", "text": "hi", "appBlock": { "pinned": true } },
				{ "type": "toolCall", "id": "1", "name": "read", "arguments": {}, "appCall": 1 },
			],
			"api": "openai-responses",
			"provider": "openai",
			"model": "m",
			"usage": {
				"input": 1, "output": 2, "cacheRead": 0, "cacheWrite": 0, "totalTokens": 3,
				"cost": { "input": 0, "output": 0, "cacheRead": 0, "cacheWrite": 0, "total": 0, "appCost": "x" },
				"appUsage": [1],
			},
			"stopReason": "stop",
			"deferred": { "provider": "openai", "modelId": "m", "api": "openai-responses", "id": "r", "appDeferred": null },
			"timestamp": 1,
			"appMessage": "kept",
		});
		let parsed: Message = serde_json::from_value(message.clone()).unwrap();
		let Message::Assistant(assistant) = &parsed else {
			panic!("expected an assistant message");
		};
		assert_eq!(assistant.extra.get("appMessage"), Some(&json!("kept")));
		let AssistantContent::Text(text) = &assistant.content[0] else {
			panic!("expected a text block");
		};
		assert_eq!(text.extra.get("appBlock"), Some(&json!({ "pinned": true })));
		assert_eq!(json_stringify(&parsed), json_stringify(&message));

		let block = json!({ "type": "image", "data": "AA==", "mimeType": "image/png", "appImage": 2 });
		let image: ImageContent = serde_json::from_value(block.clone()).unwrap();
		assert_eq!(json_stringify(&image), json_stringify(&block));

		let tool = json!({ "name": "read", "description": "Read", "parameters": {}, "appTool": true });
		assert_eq!(
			serde_json::to_value(serde_json::from_value::<Tool>(tool.clone()).unwrap()).unwrap(),
			tool
		);
	}

	#[test]
	fn any_model_reads_its_type_and_defaults_to_chat() {
		let model = json!({
			"id": "m",
			"name": "M",
			"api": "openai-completions",
			"provider": "openai",
			"baseUrl": "https://example.test",
			"input": ["text"],
			"cost": { "input": 1, "output": 2, "cacheRead": 0, "cacheWrite": 0 },
			"reasoning": false,
			"contextWindow": 1000,
			"maxTokens": 100,
			"compat": { "supportsStore": false, "maxTokensField": "max_tokens" },
		});
		let AnyModel::Chat(chat) = serde_json::from_value::<AnyModel>(model.clone()).unwrap() else {
			panic!("expected a chat model");
		};
		assert_eq!(chat.id, "m");
		assert_eq!(chat.openai_completions_compat().supports_store, Some(false));
		assert_eq!(json_stringify(&chat), json_stringify(&model));

		let mut extended = model.clone();
		extended["appModel"] = json!({ "tier": 1 });
		assert_eq!(
			json_stringify(&serde_json::from_value::<AnyModel>(extended.clone()).unwrap()),
			json_stringify(&extended)
		);

		let mut untyped = model.clone();
		untyped["type"] = Value::Null;
		untyped["compat"] = json!({ "supportsStore": false, "thinkingFormat": "future-format" });
		let AnyModel::Chat(chat) = serde_json::from_value::<AnyModel>(untyped).unwrap() else {
			panic!("expected a chat model");
		};
		assert_eq!(chat.openai_completions_compat().supports_store, Some(false));
		assert_eq!(chat.openai_completions_compat().thinking_format, None);

		let mut future = model.clone();
		future["type"] = json!("video");
		let parsed = serde_json::from_value::<AnyModel>(future.clone()).unwrap();
		assert_eq!(parsed, AnyModel::Other(future.clone()));
		assert_eq!(json_stringify(&parsed), json_stringify(&future));
	}
}
