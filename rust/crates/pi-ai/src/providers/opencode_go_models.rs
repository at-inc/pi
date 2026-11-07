//! Port of `packages/ai/src/providers/opencode-go.models.ts`.
//! PORT-STATUS: ported
//!
//! The TS file imports `./data/opencode-go.json`; `build.rs` copies it into `OUT_DIR` and this file embeds it. Each
//! catalog is parsed and flattened on first use.

use std::sync::LazyLock;

use crate::model_catalog::{
	ChatModelCatalog, ClassifierModelCatalog, ImageModelCatalog, ModelGroups, flatten_chat_model_catalog,
	flatten_classifier_model_catalog, flatten_image_model_catalog,
};

static VALUES: LazyLock<ModelGroups> = LazyLock::new(|| {
	serde_json::from_str(include_str!(concat!(env!("OUT_DIR"), "/model-data/opencode-go.json")))
		.expect("generated model data opencode-go.json is valid JSON")
});

pub static OPENCODE_GO_MODELS: LazyLock<ChatModelCatalog> =
	LazyLock::new(|| flatten_chat_model_catalog("opencode-go", &VALUES));

pub static OPENCODE_GO_IMAGE_MODELS: LazyLock<ImageModelCatalog> =
	LazyLock::new(|| flatten_image_model_catalog("opencode-go", &VALUES));

pub static OPENCODE_GO_CLASSIFIER_MODELS: LazyLock<ClassifierModelCatalog> =
	LazyLock::new(|| flatten_classifier_model_catalog("opencode-go", &VALUES));
