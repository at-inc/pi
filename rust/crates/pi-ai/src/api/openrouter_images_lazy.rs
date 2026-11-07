//! Port of `packages/ai/src/api/openrouter-images.lazy.ts`.
//! PORT-STATUS: reviewed

use std::rc::Rc;

use pi_host::{LocalBoxFuture, PiResult};

use super::openrouter_images;
use crate::types::{AssistantImages, ImageModel, ImagesContext, ProviderImages, ProviderImagesOptions};

struct LazyImages;

impl ProviderImages for LazyImages {
	fn generate_images(
		&self,
		model: &ImageModel,
		context: &ImagesContext,
		options: Option<ProviderImagesOptions>,
	) -> LocalBoxFuture<'static, PiResult<AssistantImages>> {
		openrouter_images::generate_images(model, context, options.map(|options| options.base))
	}
}

/// `openrouterImagesApi()`: generates images through the `openrouter-images` module.
pub fn openrouter_images_api() -> Rc<dyn ProviderImages> {
	Rc::new(LazyImages)
}
