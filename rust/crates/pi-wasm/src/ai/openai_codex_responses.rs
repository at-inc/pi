//! Exports of `api/openai-codex-responses` and `api/openai-codex-responses.lazy`.

use std::rc::Rc;

use pi_ai::api::{openai_codex_responses, openai_codex_responses_lazy};
use wasm_bindgen::prelude::*;

use super::models::NativeStreams;

/// The module's own implementation as `ProviderStreams`, behind its `stream` and `streamSimple` exports.
#[wasm_bindgen(js_name = aiOpenAICodexResponsesStreams)]
pub fn streams() -> NativeStreams {
	NativeStreams::new(Rc::new(openai_codex_responses::OpenAICodexResponsesApi))
}

/// `openAICodexResponsesApi()`: the module as lazily loaded `ProviderStreams`.
#[wasm_bindgen(js_name = aiOpenAICodexResponsesApi)]
pub fn api() -> NativeStreams {
	NativeStreams::new(openai_codex_responses_lazy::openai_codex_responses_api())
}
