//! Port of `packages/ai/src/api/openai-codex-images.lazy.ts`.
//! PORT-STATUS: reviewed
//!
//! Rust has no dynamic import: `generateImages` calls the statically linked module. The module takes `ImagesOptions`,
//! so the extra option keys are not passed on, as the TS module reads none.

use std::rc::Rc;

use pi_host::{LocalBoxFuture, PiResult};

use crate::api::openai_codex_images::generate_images;
use crate::types::{AssistantImages, ImageModel, ImagesContext, ProviderImages, ProviderImagesOptions};

struct OpenAICodexImagesLazy;

impl ProviderImages for OpenAICodexImagesLazy {
	fn generate_images(
		&self,
		model: &ImageModel,
		context: &ImagesContext,
		options: Option<ProviderImagesOptions>,
	) -> LocalBoxFuture<'static, PiResult<AssistantImages>> {
		generate_images(model, context, options.map(|options| options.base))
	}
}

/// `openAICodexImagesApi()`.
pub fn openai_codex_images_api() -> Rc<dyn ProviderImages> {
	Rc::new(OpenAICodexImagesLazy)
}
