//! Port of `packages/ai/src/images-api-registry.ts`.
//! PORT-STATUS: ported
//!
//! The registry is per thread. `generateImages` functions take `ProviderImagesOptions` (`ImagesOptions` plus the
//! extra keys of `images.ts`'s `options`), which is how the TS object reaches the API module; the `TOptions`
//! parameter only narrows types. The wrapper's `Mismatched api` error rejects the returned future, where TS throws
//! it synchronously.
//!
//! TS registers the builtin image APIs when `images.ts` or `compat.ts` is imported, before any app code runs. Rust
//! has no import side effects, so the first registry access in a thread registers them first; a caller's
//! registration for the same API then replaces the builtin, as it does after the TS import.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use indexmap::IndexMap;
use pi_host::PiError;

use crate::providers::images::register_builtins::register_built_in_images_api_providers;
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

thread_local! {
	static BUILTINS_REGISTERED: Cell<bool> = const { Cell::new(false) };
}

fn ensure_builtins_registered() {
	if !BUILTINS_REGISTERED.replace(true) {
		register_built_in_images_api_providers();
	}
}

pub fn register_images_api_provider(provider: ImagesApiProvider, source_id: Option<&str>) {
	ensure_builtins_registered();
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
	ensure_builtins_registered();
	IMAGES_API_PROVIDER_REGISTRY.with(|registry| registry.borrow().get(api).map(|entry| entry.provider.clone()))
}
