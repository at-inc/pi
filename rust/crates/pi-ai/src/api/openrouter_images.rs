//! Port of `packages/ai/src/api/openrouter-images.ts`.
//! PORT-STATUS: pending

// PORT-STUB: w3-bedrock-misc
use pi_host::{LocalBoxFuture, PiResult};

use crate::types::{AssistantImages, ImageModel, ImagesContext, ImagesOptions};

pub fn generate_images(
	_model: &ImageModel,
	_context: &ImagesContext,
	_options: Option<ImagesOptions>,
) -> LocalBoxFuture<'static, PiResult<AssistantImages>> {
	todo!("PORT-STUB")
}
