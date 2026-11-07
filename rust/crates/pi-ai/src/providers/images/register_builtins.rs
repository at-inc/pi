//! Port of `packages/ai/src/providers/images/register-builtins.ts`.
//! PORT-STATUS: ported
//!
//! Differences from the TS:
//! - Rust has no dynamic import, so the cached module promises are direct calls into `api/openai_codex_images.rs` and
//!   `api/openrouter_images.rs`; a failure of the call becomes the error result, like a failed import or rejection.
//! - The TS registers the built-ins when the module loads; Rust has no module side effects, so callers run
//!   `register_built_in_images_api_providers()` (`images.rs` does).
//! - The registry takes `ImagesFunction<ProviderImagesOptions>`; the API modules take `ImagesOptions`, so the
//!   registered functions pass `options.base` on.

use std::rc::Rc;

use pi_host::{LocalBoxFuture, PiError, PiResult};

use crate::api::openai_codex_images;
use crate::api::openrouter_images;
use crate::images_api_registry::{ImagesApiProvider, register_images_api_provider};
use crate::types::{
	AssistantImages, ImageModel, ImagesContext, ImagesOptions, ImagesStopReason, ProviderImagesOptions,
};

fn create_lazy_load_error_images(model: &ImageModel, error: &PiError) -> AssistantImages {
	AssistantImages {
		api: model.api.clone(),
		provider: model.provider.clone(),
		model: model.id.clone(),
		output: Vec::new(),
		response_id: None,
		usage: None,
		stop_reason: ImagesStopReason::Error,
		error_message: Some(error.message.clone()),
		timestamp: pi_host::now_ms(),
	}
}

pub fn generate_images_openai_codex(
	model: &ImageModel,
	context: &ImagesContext,
	options: Option<ImagesOptions>,
) -> LocalBoxFuture<'static, PiResult<AssistantImages>> {
	let (model, context) = (model.clone(), context.clone());
	Box::pin(async move {
		match openai_codex_images::generate_images(&model, &context, options).await {
			Ok(images) => Ok(images),
			Err(error) => Ok(create_lazy_load_error_images(&model, &error)),
		}
	})
}

pub fn generate_images_open_router(
	model: &ImageModel,
	context: &ImagesContext,
	options: Option<ImagesOptions>,
) -> LocalBoxFuture<'static, PiResult<AssistantImages>> {
	let (model, context) = (model.clone(), context.clone());
	Box::pin(async move {
		match openrouter_images::generate_images(&model, &context, options).await {
			Ok(images) => Ok(images),
			Err(error) => Ok(create_lazy_load_error_images(&model, &error)),
		}
	})
}

pub fn register_built_in_images_api_providers() {
	register_images_api_provider(
		ImagesApiProvider {
			api: "openai-codex-images".into(),
			generate_images: Rc::new(|model, context, options: Option<ProviderImagesOptions>| {
				generate_images_openai_codex(model, context, options.map(|options| options.base))
			}),
		},
		None,
	);
	register_images_api_provider(
		ImagesApiProvider {
			api: "openrouter-images".into(),
			generate_images: Rc::new(|model, context, options: Option<ProviderImagesOptions>| {
				generate_images_open_router(model, context, options.map(|options| options.base))
			}),
		},
		None,
	);
}
