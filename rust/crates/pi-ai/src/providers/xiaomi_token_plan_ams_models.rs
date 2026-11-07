//! Port of `packages/ai/src/providers/xiaomi-token-plan-ams.models.ts`.
//! PORT-STATUS: ported
//!
//! The TS file imports `./data/xiaomi-token-plan-ams.json`; `build.rs` copies it into `OUT_DIR` and this file embeds it. Each
//! catalog is parsed and flattened on first use.

use std::sync::LazyLock;

use crate::model_catalog::{
	ChatModelCatalog, ClassifierModelCatalog, ImageModelCatalog, ModelGroups, flatten_chat_model_catalog,
	flatten_classifier_model_catalog, flatten_image_model_catalog,
};

static VALUES: LazyLock<ModelGroups> = LazyLock::new(|| {
	serde_json::from_str(include_str!(concat!(
		env!("OUT_DIR"),
		"/model-data/xiaomi-token-plan-ams.json"
	)))
	.expect("generated model data xiaomi-token-plan-ams.json is valid JSON")
});

pub static XIAOMI_TOKEN_PLAN_AMS_IMAGE_MODELS: LazyLock<ChatModelCatalog> =
	LazyLock::new(|| flatten_chat_model_catalog("xiaomi-token-plan-ams", &VALUES));

pub static XIAOMI_TOKEN_PLAN_AMS_IMAGE_IMAGE_MODELS: LazyLock<ImageModelCatalog> =
	LazyLock::new(|| flatten_image_model_catalog("xiaomi-token-plan-ams", &VALUES));

pub static XIAOMI_TOKEN_PLAN_AMS_IMAGE_CLASSIFIER_MODELS: LazyLock<ClassifierModelCatalog> =
	LazyLock::new(|| flatten_classifier_model_catalog("xiaomi-token-plan-ams", &VALUES));
