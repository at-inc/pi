//! Exports of `api/azure-openai-responses` and `api/azure-openai-responses.lazy`.

use std::rc::Rc;

use pi_ai::api::{azure_openai_responses, azure_openai_responses_lazy};
use wasm_bindgen::prelude::*;

use super::models::NativeStreams;

/// The module's own implementation as `ProviderStreams`, behind its `stream` and `streamSimple` exports.
#[wasm_bindgen(js_name = aiAzureOpenAIResponsesStreams)]
pub fn streams() -> NativeStreams {
	NativeStreams::new(Rc::new(azure_openai_responses::AzureOpenAIResponsesApi))
}

/// `azureOpenAIResponsesApi()`: the module as lazily loaded `ProviderStreams`.
#[wasm_bindgen(js_name = aiAzureOpenAIResponsesApi)]
pub fn api() -> NativeStreams {
	NativeStreams::new(azure_openai_responses_lazy::azure_openai_responses_api())
}
