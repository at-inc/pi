//! Port of `packages/ai/src/providers/qwen-token-plan-individual.models.ts`.
//! PORT-STATUS: reviewed
//!
//! The TS file imports `./data/qwen-token-plan-individual.json`; `build.rs` copies it into `OUT_DIR` and this file embeds it. Each
//! catalog is parsed and flattened on first use.

use std::sync::LazyLock;

use crate::model_catalog::{
	ChatModelCatalog, ClassifierModelCatalog, ImageModelCatalog, ModelGroups, flatten_chat_model_catalog,
	flatten_classifier_model_catalog, flatten_image_model_catalog,
};

static VALUES: LazyLock<ModelGroups> = LazyLock::new(|| {
	serde_json::from_str(include_str!(concat!(
		env!("OUT_DIR"),
		"/model-data/qwen-token-plan-individual.json"
	)))
	.expect("generated model data qwen-token-plan-individual.json is valid JSON")
});

pub static QWEN_TOKEN_PLAN_INDIVIDUAL_MODELS: LazyLock<ChatModelCatalog> =
	LazyLock::new(|| flatten_chat_model_catalog("qwen-token-plan-individual", &VALUES));

pub static QWEN_TOKEN_PLAN_INDIVIDUAL_IMAGE_MODELS: LazyLock<ImageModelCatalog> =
	LazyLock::new(|| flatten_image_model_catalog("qwen-token-plan-individual", &VALUES));

pub static QWEN_TOKEN_PLAN_INDIVIDUAL_CLASSIFIER_MODELS: LazyLock<ClassifierModelCatalog> =
	LazyLock::new(|| flatten_classifier_model_catalog("qwen-token-plan-individual", &VALUES));
