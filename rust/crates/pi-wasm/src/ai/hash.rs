//! Exports of `utils/hash`.

use wasm_bindgen::prelude::*;

/// `shortHash(str)`.
#[wasm_bindgen(js_name = aiShortHash)]
pub fn short_hash(text: &str) -> String {
	pi_ai::utils::hash::short_hash(text)
}
