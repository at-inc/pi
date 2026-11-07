//! Exports of `api/mistral-conversations` and `api/mistral-conversations.lazy`.

use std::rc::Rc;

use pi_ai::api::{mistral_conversations, mistral_conversations_lazy};
use wasm_bindgen::prelude::*;

use super::models::NativeStreams;

/// The module's own implementation as `ProviderStreams`, behind its `stream` and `streamSimple` exports.
#[wasm_bindgen(js_name = aiMistralConversationsStreams)]
pub fn streams() -> NativeStreams {
	NativeStreams::new(Rc::new(mistral_conversations::MistralConversationsStreams))
}

/// `mistralConversationsApi()`: the module as lazily loaded `ProviderStreams`.
#[wasm_bindgen(js_name = aiMistralConversationsApi)]
pub fn api() -> NativeStreams {
	NativeStreams::new(mistral_conversations_lazy::mistral_conversations_api())
}
