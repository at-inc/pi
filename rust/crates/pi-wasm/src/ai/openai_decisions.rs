//! Exports of `api/openai-decisions` and `api/openai-decisions.lazy`.

use pi_ai::api::openai_decisions_lazy;
use wasm_bindgen::prelude::*;

use super::models::NativeClassifier;

/// `openAIDecisionsApi()`: the module as lazily loaded `ProviderClassifier`; the facade's `classify` calls it too.
#[wasm_bindgen(js_name = aiOpenAIDecisionsApi)]
pub fn api() -> NativeClassifier {
	NativeClassifier::new(openai_decisions_lazy::openai_decisions_api())
}
