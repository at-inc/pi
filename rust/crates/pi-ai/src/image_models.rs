//! Port of `packages/ai/src/image-models.ts`.
//! PORT-STATUS: ported
//!
//! Compat reads of the generated catalog restricted to image models. New code
//! uses `Models.getModelOfType("image", ...)` or `getBuiltinImageModel()` from `providers/all`.
//!
//! The TS types narrowing providers and ids to the generated catalog (`BuiltinImageProvider`,
//! `BuiltinImageModel`) have no runtime effect; providers and ids are strings, and `getImageModel` returns `None`
//! for an unknown pair where TS returns `undefined` typed as a model.

use std::sync::LazyLock;

use indexmap::IndexMap;

use crate::models_generated::IMAGE_MODELS;
use crate::types::ImageModel;

static IMAGE_MODELS_BY_PROVIDER: LazyLock<IndexMap<String, IndexMap<String, ImageModel>>> = LazyLock::new(|| {
	let mut image_models_by_provider = IndexMap::new();
	for (provider, models) in IMAGE_MODELS.iter() {
		let mut image_models = IndexMap::new();
		for model in models.values() {
			image_models.insert(model.id.clone(), model.clone());
		}
		if !image_models.is_empty() {
			image_models_by_provider.insert((*provider).to_string(), image_models);
		}
	}
	image_models_by_provider
});

/// @deprecated Static catalog read. Use `getBuiltinImageModel` from "@at-inc/pi-ai/providers/all" or `Models.getModelOfType("image", ...)`.
pub fn get_image_model(provider: &str, model_id: &str) -> Option<ImageModel> {
	IMAGE_MODELS_BY_PROVIDER
		.get(provider)
		.and_then(|models| models.get(model_id))
		.cloned()
}

/// @deprecated Static catalog read. Use `Models.getProviders()`.
pub fn get_image_providers() -> Vec<String> {
	IMAGE_MODELS_BY_PROVIDER.keys().cloned().collect()
}

/// @deprecated Static catalog read. Use `getBuiltinImageModels` from "@at-inc/pi-ai/providers/all" or `Models.getModelsOfType("image")`.
pub fn get_image_models(provider: &str) -> Vec<ImageModel> {
	IMAGE_MODELS_BY_PROVIDER
		.get(provider)
		.map(|models| models.values().cloned().collect())
		.unwrap_or_default()
}
