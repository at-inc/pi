//! Port of `packages/ai/src/images.ts`.
//! PORT-STATUS: ported
//!
//! The TS import of `providers/images/register-builtins.ts` is the registry's first-access registration (see
//! `images_api_registry.rs`).

use pi_host::{LocalBoxFuture, PiError, PiResult};

use crate::images_api_registry::{ImagesApiProviderInternal, get_images_api_provider};
use crate::types::{AssistantImages, ImageModel, ImagesContext, ProviderImagesOptions};

fn resolve_images_api_provider(api: &str) -> PiResult<ImagesApiProviderInternal> {
	get_images_api_provider(api).ok_or_else(|| PiError::new(format!("No API provider registered for api: {api}")))
}

/// Global image generation dispatched on `model.api` through the images api
/// registry. Auth must be passed explicitly via `options.apiKey`; prefer
/// `Models.generateImages()`, which resolves provider auth.
pub fn generate_images(
	model: &ImageModel,
	context: &ImagesContext,
	options: Option<ProviderImagesOptions>,
) -> LocalBoxFuture<'static, PiResult<AssistantImages>> {
	match resolve_images_api_provider(&model.api) {
		Ok(provider) => (provider.generate_images)(model, context, options),
		Err(error) => Box::pin(async move { Err(error) }),
	}
}
