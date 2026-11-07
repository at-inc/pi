//! Exports of `bun-oauth`.

use wasm_bindgen::prelude::*;

/// `registerBunOAuthFlows()`.
#[wasm_bindgen(js_name = aiRegisterBunOAuthFlows)]
pub fn register_bun_oauth_flows() {
	pi_ai::bun_oauth::register_bun_oauth_flows();
}
