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

/// `setBedrockProviderModule(module)`: `module` becomes the implementation `bedrockConverseStreamApi()` loads. The
/// facade passes `native` when `module` is a facade value, so its Rust implementation is used directly; any other
/// object is a JS implementation.
#[wasm_bindgen(js_name = aiSetBedrockProviderModule)]
pub fn set_bedrock_provider_module(module: JsValue, native: Option<NativeStreams>) {
	bedrock_converse_stream_lazy::set_bedrock_provider_module(super::models::provider_streams(native, module));
}
