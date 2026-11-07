//! Exports of `api/openai-responses` and `api/openai-responses.lazy`.

use std::rc::Rc;

use pi_ai::api::{openai_responses, openai_responses_lazy};
use wasm_bindgen::prelude::*;

use super::models::NativeStreams;

/// The module's own implementation as `ProviderStreams`, behind its `stream` and `streamSimple` exports.
#[wasm_bindgen(js_name = aiOpenAIResponsesStreams)]
pub fn streams() -> NativeStreams {
	NativeStreams::new(Rc::new(openai_responses::OpenAIResponsesApi))
}

/// `openAIResponsesApi()`: the module as lazily loaded `ProviderStreams`.
#[wasm_bindgen(js_name = aiOpenAIResponsesApi)]
pub fn api() -> NativeStreams {
	NativeStreams::new(openai_responses_lazy::openai_responses_api())
}
