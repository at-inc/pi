//! Exports of `providers/opencode-headers`.

use pi_ai::providers::opencode_headers;
use wasm_bindgen::prelude::*;

use super::models::{NativeStreams, provider_streams};

/// `withOpenCodeSessionHeader(streams)`, over a facade `*Api()` value or a JS implementation.
#[wasm_bindgen(js_name = aiWithOpenCodeSessionHeader)]
pub fn with_open_code_session_header(native: Option<NativeStreams>, object: JsValue) -> NativeStreams {
	NativeStreams::new(opencode_headers::with_open_code_session_header(provider_streams(
		native, object,
	)))
}
