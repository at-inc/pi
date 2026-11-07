//! Exports of `api/google-vertex` and `api/google-vertex.lazy`.

use std::rc::Rc;

use pi_ai::api::{google_vertex, google_vertex_lazy};
use wasm_bindgen::prelude::*;

use super::models::NativeStreams;

/// The module's own implementation as `ProviderStreams`, behind its `stream` and `streamSimple` exports.
#[wasm_bindgen(js_name = aiGoogleVertexStreams)]
pub fn streams() -> NativeStreams {
	NativeStreams::new(Rc::new(google_vertex::GoogleVertexStreams))
}

/// `googleVertexApi()`: the module as lazily loaded `ProviderStreams`.
#[wasm_bindgen(js_name = aiGoogleVertexApi)]
pub fn api() -> NativeStreams {
	NativeStreams::new(google_vertex_lazy::google_vertex_api())
}
