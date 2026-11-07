//! Exports of `api/openrouter-images` and `api/openrouter-images.lazy`.

use pi_ai::api::openrouter_images_lazy;
use wasm_bindgen::prelude::*;

use super::models::NativeImages;

/// `openrouterImagesApi()`: the module as lazily loaded `ProviderImages`; the facade's `generateImages` calls it too.
#[wasm_bindgen(js_name = aiOpenrouterImagesApi)]
pub fn api() -> NativeImages {
	NativeImages::new(openrouter_images_lazy::openrouter_images_api())
}
