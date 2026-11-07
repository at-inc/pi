//! Port of `packages/ai/src/legacy-api-aliases.ts`.
//! PORT-STATUS: reviewed
//!
//! The TS module builds each lazy API once at import; the port builds it once per thread on first use, which is not
//! observable because building one has no side effects. The TS casts each `stream` to the API's own options type
//! (`AnthropicOptions`, ...); those types belong to the wave-3 API modules, so `stream_*` takes
//! `ProviderStreamOptions`, whose `extra` map carries the API-specific keys. The TS `@deprecated` tags are
//! `#[deprecated]` attributes.

use pi_host::PiResult;

use crate::types::{Model, ProviderStreamOptions, SimpleStreamOptions, TranscriptContext};
use crate::utils::event_stream::AssistantMessageEventStream;

macro_rules! legacy_api_aliases {
	($streams:ident = $api:expr, #[$stream_attr:meta] $stream:ident, #[$simple_attr:meta] $stream_simple:ident) => {
		thread_local! {
			static $streams: std::rc::Rc<dyn crate::types::ProviderStreams> = $api;
		}

		#[$stream_attr]
		pub fn $stream(
			model: &Model,
			context: &TranscriptContext,
			options: Option<ProviderStreamOptions>,
		) -> PiResult<AssistantMessageEventStream> {
			$streams.with(|streams| streams.stream(model, context, options))
		}

		#[$simple_attr]
		pub fn $stream_simple(
			model: &Model,
			context: &TranscriptContext,
			options: Option<SimpleStreamOptions>,
		) -> PiResult<AssistantMessageEventStream> {
			$streams.with(|streams| streams.stream_simple(model, context, options))
		}
	};
}

legacy_api_aliases!(
	ANTHROPIC_MESSAGES_STREAMS = crate::api::anthropic_messages_lazy::anthropic_messages_api(),
	#[deprecated(
		note = "Use `stream` from `@at-inc/pi-ai/api/anthropic-messages` or `anthropicMessagesApi().stream`."
	)]
	stream_anthropic,
	#[deprecated(
		note = "Use `streamSimple` from `@at-inc/pi-ai/api/anthropic-messages` or `anthropicMessagesApi().streamSimple`."
	)]
	stream_simple_anthropic
);

legacy_api_aliases!(
	AZURE_OPENAI_RESPONSES_STREAMS = crate::api::azure_openai_responses_lazy::azure_openai_responses_api(),
	#[deprecated(
		note = "Use `stream` from `@at-inc/pi-ai/api/azure-openai-responses` or `azureOpenAIResponsesApi().stream`."
	)]
	stream_azure_openai_responses,
	#[deprecated(
		note = "Use `streamSimple` from `@at-inc/pi-ai/api/azure-openai-responses` or `azureOpenAIResponsesApi().streamSimple`."
	)]
	stream_simple_azure_openai_responses
);

legacy_api_aliases!(
	GOOGLE_GENERATIVE_AI_STREAMS = crate::api::google_generative_ai_lazy::google_generative_ai_api(),
	#[deprecated(
		note = "Use `stream` from `@at-inc/pi-ai/api/google-generative-ai` or `googleGenerativeAIApi().stream`."
	)]
	stream_google,
	#[deprecated(
		note = "Use `streamSimple` from `@at-inc/pi-ai/api/google-generative-ai` or `googleGenerativeAIApi().streamSimple`."
	)]
	stream_simple_google
);

legacy_api_aliases!(
	GOOGLE_VERTEX_STREAMS = crate::api::google_vertex_lazy::google_vertex_api(),
	#[deprecated(note = "Use `stream` from `@at-inc/pi-ai/api/google-vertex` or `googleVertexApi().stream`.")]
	stream_google_vertex,
	#[deprecated(
		note = "Use `streamSimple` from `@at-inc/pi-ai/api/google-vertex` or `googleVertexApi().streamSimple`."
	)]
	stream_simple_google_vertex
);

legacy_api_aliases!(
	MISTRAL_CONVERSATIONS_STREAMS = crate::api::mistral_conversations_lazy::mistral_conversations_api(),
	#[deprecated(
		note = "Use `stream` from `@at-inc/pi-ai/api/mistral-conversations` or `mistralConversationsApi().stream`."
	)]
	stream_mistral,
	#[deprecated(
		note = "Use `streamSimple` from `@at-inc/pi-ai/api/mistral-conversations` or `mistralConversationsApi().streamSimple`."
	)]
	stream_simple_mistral
);

legacy_api_aliases!(
	OPENAI_CODEX_RESPONSES_STREAMS = crate::api::openai_codex_responses_lazy::openai_codex_responses_api(),
	#[deprecated(
		note = "Use `stream` from `@at-inc/pi-ai/api/openai-codex-responses` or `openAICodexResponsesApi().stream`."
	)]
	stream_openai_codex_responses,
	#[deprecated(
		note = "Use `streamSimple` from `@at-inc/pi-ai/api/openai-codex-responses` or `openAICodexResponsesApi().streamSimple`."
	)]
	stream_simple_openai_codex_responses
);

legacy_api_aliases!(
	OPENAI_COMPLETIONS_STREAMS = crate::api::openai_completions_lazy::openai_completions_api(),
	#[deprecated(
		note = "Use `stream` from `@at-inc/pi-ai/api/openai-completions` or `openAICompletionsApi().stream`."
	)]
	stream_openai_completions,
	#[deprecated(
		note = "Use `streamSimple` from `@at-inc/pi-ai/api/openai-completions` or `openAICompletionsApi().streamSimple`."
	)]
	stream_simple_openai_completions
);

legacy_api_aliases!(
	OPENAI_RESPONSES_STREAMS = crate::api::openai_responses_lazy::openai_responses_api(),
	#[deprecated(note = "Use `stream` from `@at-inc/pi-ai/api/openai-responses` or `openAIResponsesApi().stream`.")]
	stream_openai_responses,
	#[deprecated(
		note = "Use `streamSimple` from `@at-inc/pi-ai/api/openai-responses` or `openAIResponsesApi().streamSimple`."
	)]
	stream_simple_openai_responses
);
