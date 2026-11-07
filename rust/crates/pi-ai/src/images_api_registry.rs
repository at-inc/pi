//! Port of `packages/ai/src/images-api-registry.ts`.
//! PORT-STATUS: ported
//!
//! The registry is per thread. `generateImages` functions take `ProviderImagesOptions` (`ImagesOptions` plus the
//! extra keys of `images.ts`'s `options`), which is how the TS object reaches the API module; the `TOptions`
//! parameter only narrows types.

use std::cell::RefCell;
use std::rc::Rc;

use indexmap::IndexMap;
use pi_host::PiError;

use crate::types::{ImageApi, ImagesFunction, ProviderImagesOptions};

/// `ImagesApiFunction`.
pub type ImagesApiFunction = ImagesFunction<ProviderImagesOptions>;

#[derive(Clone)]
pub struct ImagesApiProvider {
	pub api: ImageApi,
	pub generate_images: ImagesFunction<ProviderImagesOptions>,
}

/// `ImagesApiProviderInternal`.
#[derive(Clone)]
pub struct ImagesApiProviderInternal {
	pub api: ImageApi,
	pub generate_images: ImagesApiFunction,
}

#[derive(Clone)]
struct RegisteredImagesApiProvider {
	provider: ImagesApiProviderInternal,
	#[allow(dead_code, reason = "kept like the TS registry entry; nothing reads it yet")]
	source_id: Option<String>,
}

thread_local! {
	static IMAGES_API_PROVIDER_REGISTRY: RefCell<IndexMap<String, RegisteredImagesApiProvider>> =
		RefCell::new(IndexMap::new());
}

fn wrap_generate_images(api: ImageApi, generate_images: ImagesFunction<ProviderImagesOptions>) -> ImagesApiFunction {
	Rc::new(move |model, context, options| {
		if model.api != api {
			let error = PiError::new(format!("Mismatched api: {} expected {}", model.api, api));
			return Box::pin(async move { Err(error) });
		}
		generate_images(model, context, options)
	})
}

pub fn register_images_api_provider(provider: ImagesApiProvider, source_id: Option<&str>) {
	IMAGES_API_PROVIDER_REGISTRY.with(|registry| {
		registry.borrow_mut().insert(
			provider.api.to_string(),
			RegisteredImagesApiProvider {
				provider: ImagesApiProviderInternal {
					api: provider.api.clone(),
					generate_images: wrap_generate_images(provider.api, provider.generate_images),
				},
				source_id: source_id.map(str::to_string),
			},
		);
	});
}

pub fn get_images_api_provider(api: &str) -> Option<ImagesApiProviderInternal> {
	IMAGES_API_PROVIDER_REGISTRY.with(|registry| registry.borrow().get(api).map(|entry| entry.provider.clone()))
}
