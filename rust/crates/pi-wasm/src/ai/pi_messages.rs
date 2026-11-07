//! Exports of `api/pi-messages` and `api/pi-messages.lazy`.

use pi_ai::api::{pi_messages, pi_messages_lazy};
use wasm_bindgen::prelude::*;

use super::models::NativeStreams;

/// The module's own implementation as `ProviderStreams`, behind its `stream` and `streamSimple` exports.
#[wasm_bindgen(js_name = aiPiMessagesStreams)]
pub fn streams() -> NativeStreams {
	NativeStreams::new(pi_messages::pi_messages_streams())
}

/// `piMessagesApi()`: the module as lazily loaded `ProviderStreams`.
#[wasm_bindgen(js_name = aiPiMessagesApi)]
pub fn api() -> NativeStreams {
	NativeStreams::new(pi_messages_lazy::pi_messages_api())
}
