//! Exports of `api/openai-codex-images` and `api/openai-codex-images.lazy`.

use pi_ai::api::openai_codex_images_lazy;
use wasm_bindgen::prelude::*;

use super::models::NativeImages;

/// `openAICodexImagesApi()`: the module as lazily loaded `ProviderImages`; the facade's `generateImages` calls it too.
#[wasm_bindgen(js_name = aiOpenAICodexImagesApi)]
pub fn api() -> NativeImages {
	NativeImages::new(openai_codex_images_lazy::openai_codex_images_api())
}
