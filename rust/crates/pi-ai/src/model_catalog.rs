//! Port of `packages/ai/src/model-catalog.ts`.
//! PORT-STATUS: ported
//!
//! The mapped catalog types (`ChatModelCatalog`, `ImageModelCatalog`, `ClassifierModelCatalog` and their key helpers)
//! only narrow ids and APIs at the type level; each catalog is an `IndexMap` from model id to the model, in the order
//! `Object.fromEntries` produces. The flatten functions take the parsed generated JSON (`ModelGroups`). Catalog data
//! is generated at build time, so an entry that does not deserialize as its type is a build defect and panics.

use indexmap::IndexMap;
use serde::de::DeserializeOwned;
use serde_json::Value;

use crate::types::{ClassifierModel, ImageModel, Model, ModelType};
use crate::utils::model_operations::model_type_name;

/// `Record<string, Record<string, object>>`: the generated catalog JSON of one provider, grouped by API.
pub type ModelGroups = Value;

/// Chat models of one provider keyed by model id.
pub type ChatModelCatalog = IndexMap<String, Model>;
/// Image models of one provider keyed by model id.
pub type ImageModelCatalog = IndexMap<String, ImageModel>;
/// Classifier models of one provider keyed by model id.
pub type ClassifierModelCatalog = IndexMap<String, ClassifierModel>;

fn flatten_model_catalog<T: DeserializeOwned>(groups: &ModelGroups, model_type: ModelType) -> IndexMap<String, T> {
	let mut catalog = IndexMap::new();
	let groups = groups.as_object().into_iter().flat_map(|groups| groups.values());
	for model in groups.filter_map(Value::as_object).flat_map(|models| models.values()) {
		if model.get("type").and_then(Value::as_str) != Some(model_type_name(model_type)) {
			continue;
		}
		let id = model.get("id").and_then(Value::as_str).unwrap_or_default().to_string();
		let parsed = serde_json::from_value(model.clone())
			.unwrap_or_else(|error| panic!("generated catalog entry {id} is not a valid model: {error}"));
		catalog.insert(id, parsed);
	}
	catalog
}

pub fn flatten_chat_model_catalog(_provider: &str, groups: &ModelGroups) -> ChatModelCatalog {
	flatten_model_catalog(groups, ModelType::Chat)
}

pub fn flatten_image_model_catalog(_provider: &str, groups: &ModelGroups) -> ImageModelCatalog {
	flatten_model_catalog(groups, ModelType::Image)
}

pub fn flatten_classifier_model_catalog(_provider: &str, groups: &ModelGroups) -> ClassifierModelCatalog {
	flatten_model_catalog(groups, ModelType::Classifier)
}
