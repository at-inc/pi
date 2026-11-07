//! Exports of `api/anthropic-messages` and `api/anthropic-messages.lazy`.

use std::rc::Rc;

use pi_ai::api::{anthropic_messages, anthropic_messages_lazy};
use wasm_bindgen::prelude::*;

use super::models::NativeStreams;

/// The module's own implementation as `ProviderStreams`, behind its `stream` and `streamSimple` exports.
#[wasm_bindgen(js_name = aiAnthropicMessagesStreams)]
pub fn streams() -> NativeStreams {
	NativeStreams::new(Rc::new(anthropic_messages::AnthropicMessagesStreams))
}

/// `anthropicMessagesApi()`: the module as lazily loaded `ProviderStreams`.
#[wasm_bindgen(js_name = aiAnthropicMessagesApi)]
pub fn api() -> NativeStreams {
	NativeStreams::new(anthropic_messages_lazy::anthropic_messages_api())
}
