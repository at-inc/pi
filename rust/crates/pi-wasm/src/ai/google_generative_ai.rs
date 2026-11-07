//! Exports of `api/google-generative-ai` and `api/google-generative-ai.lazy`.

use std::rc::Rc;

use pi_ai::api::{google_generative_ai, google_generative_ai_lazy};
use wasm_bindgen::prelude::*;

use super::models::NativeStreams;

/// The module's own implementation as `ProviderStreams`, behind its `stream` and `streamSimple` exports.
#[wasm_bindgen(js_name = aiGoogleGenerativeAIStreams)]
pub fn streams() -> NativeStreams {
	NativeStreams::new(Rc::new(google_generative_ai::GoogleGenerativeAIStreams))
}

/// `googleGenerativeAIApi()`: the module as lazily loaded `ProviderStreams`.
#[wasm_bindgen(js_name = aiGoogleGenerativeAIApi)]
pub fn api() -> NativeStreams {
	NativeStreams::new(google_generative_ai_lazy::google_generative_ai_api())
}
