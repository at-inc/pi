//! Exports of `utils/version`.

use wasm_bindgen::prelude::*;

/// `PI_AI_VERSION`.
#[wasm_bindgen(js_name = aiPiAiVersion)]
pub fn pi_ai_version() -> String {
	pi_ai::utils::version::PI_AI_VERSION.into()
}
