//! Port of `packages/ai/src/providers/ant-ling.models.ts`.
//! PORT-STATUS: ported
//!
//! The TS file imports `./data/ant-ling.json`; `build.rs` copies it into `OUT_DIR` and this file embeds it. Each
//! catalog is parsed and flattened on first use.

use std::sync::LazyLock;

use crate::model_catalog::{
	ChatModelCatalog, ClassifierModelCatalog, ImageModelCatalog, ModelGroups, flatten_chat_model_catalog,
	flatten_classifier_model_catalog, flatten_image_model_catalog,
};

static VALUES: LazyLock<ModelGroups> = LazyLock::new(|| {
	serde_json::from_str(include_str!(concat!(env!("OUT_DIR"), "/model-data/ant-ling.json")))
		.expect("generated model data ant-ling.json is valid JSON")
});

pub static ANT_LING_IMAGE_MODELS: LazyLock<ChatModelCatalog> =
	LazyLock::new(|| flatten_chat_model_catalog("ant-ling", &VALUES));

pub static ANT_LING_IMAGE_IMAGE_MODELS: LazyLock<ImageModelCatalog> =
	LazyLock::new(|| flatten_image_model_catalog("ant-ling", &VALUES));

pub static ANT_LING_IMAGE_CLASSIFIER_MODELS: LazyLock<ClassifierModelCatalog> =
	LazyLock::new(|| flatten_classifier_model_catalog("ant-ling", &VALUES));
