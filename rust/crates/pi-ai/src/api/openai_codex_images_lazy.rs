//! Port of `packages/ai/src/api/openai-codex-images.lazy.ts`.
//! PORT-STATUS: pending

// PORT-STUB: w3-openai-responses
// Until the API module is ported, the wrapper never panics: every operation fails with "API not ported".
use std::rc::Rc;

use pi_host::{LocalBoxFuture, PiError, PiResult};

use crate::types::{AssistantImages, ImageModel, ImagesContext, ProviderImages, ProviderImagesOptions};

struct NotPorted;

impl ProviderImages for NotPorted {
	fn generate_images(
		&self,
		_model: &ImageModel,
		_context: &ImagesContext,
		_options: Option<ProviderImagesOptions>,
	) -> LocalBoxFuture<'static, PiResult<AssistantImages>> {
		Box::pin(async { Err(PiError::new("API not ported")) })
	}
}

/// `openAICodexImagesApi()`: every generation rejects with `API not ported`.
pub fn openai_codex_images_api() -> Rc<dyn ProviderImages> {
	Rc::new(NotPorted)
}
