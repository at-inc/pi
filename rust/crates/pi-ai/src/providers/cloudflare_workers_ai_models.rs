//! Port of `packages/ai/src/providers/cloudflare-workers-ai.models.ts`.
//! PORT-STATUS: reviewed
//!
//! The TS file imports `./data/cloudflare-workers-ai.json`; `build.rs` copies it into `OUT_DIR` and this file embeds it. Each
//! catalog is parsed and flattened on first use.

use std::sync::LazyLock;

use crate::model_catalog::{
	ChatModelCatalog, ClassifierModelCatalog, ImageModelCatalog, ModelGroups, flatten_chat_model_catalog,
	flatten_classifier_model_catalog, flatten_image_model_catalog,
};

static VALUES: LazyLock<ModelGroups> = LazyLock::new(|| {
	serde_json::from_str(include_str!(concat!(
		env!("OUT_DIR"),
		"/model-data/cloudflare-workers-ai.json"
	)))
	.expect("generated model data cloudflare-workers-ai.json is valid JSON")
});

pub static CLOUDFLARE_WORKERS_AI_MODELS: LazyLock<ChatModelCatalog> =
	LazyLock::new(|| flatten_chat_model_catalog("cloudflare-workers-ai", &VALUES));

pub static CLOUDFLARE_WORKERS_AI_IMAGE_MODELS: LazyLock<ImageModelCatalog> =
	LazyLock::new(|| flatten_image_model_catalog("cloudflare-workers-ai", &VALUES));

pub static CLOUDFLARE_WORKERS_AI_CLASSIFIER_MODELS: LazyLock<ClassifierModelCatalog> =
	LazyLock::new(|| flatten_classifier_model_catalog("cloudflare-workers-ai", &VALUES));
