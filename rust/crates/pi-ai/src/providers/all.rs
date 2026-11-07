//! Port of `packages/ai/src/providers/all.ts`.
//! PORT-STATUS: pending

// PORT-STUB: w2-ai-core
// Signatures copied from w2-ai-core's port on `capy/rust-port-w2-ai-core` (c990cc571), which replaces this stub.
use std::rc::Rc;

use crate::models::{CreateModelsOptions, MutableModels};
use crate::types::Model;

pub fn get_builtin_model(_provider: &str, _model_id: &str) -> Option<Model> {
	todo!("PORT-STUB")
}

pub fn get_builtin_providers() -> Vec<&'static str> {
	todo!("PORT-STUB")
}

pub fn get_builtin_models(_provider: &str) -> Vec<Model> {
	todo!("PORT-STUB")
}

/// A `Models` collection with every built-in provider registered.
pub fn builtin_models(_options: Option<CreateModelsOptions>) -> Rc<dyn MutableModels> {
	todo!("PORT-STUB")
}
