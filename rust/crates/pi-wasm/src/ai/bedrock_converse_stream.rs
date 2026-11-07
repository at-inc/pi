//! Exports of `api/bedrock-converse-stream` and `api/bedrock-converse-stream.lazy`.

use std::rc::Rc;

use pi_ai::api::{bedrock_converse_stream, bedrock_converse_stream_lazy};
use wasm_bindgen::prelude::*;

use super::models::NativeStreams;

/// The module's own implementation as `ProviderStreams`, behind its `stream` and `streamSimple` exports.
#[wasm_bindgen(js_name = aiBedrockConverseStreamStreams)]
pub fn streams() -> NativeStreams {
	NativeStreams::new(Rc::new(bedrock_converse_stream::BedrockConverseStreamStreams))
}

/// `bedrockConverseStreamApi()`: the module as lazily loaded `ProviderStreams`.
#[wasm_bindgen(js_name = aiBedrockConverseStreamApi)]
pub fn api() -> NativeStreams {
	NativeStreams::new(bedrock_converse_stream_lazy::bedrock_converse_stream_api())
}
