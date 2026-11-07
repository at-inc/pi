//! Exports of `models-store`.

use std::rc::Rc;

use wasm_bindgen::prelude::*;

use super::auth::ModelsStoreHandle;

/// `new InMemoryModelsStore()`.
#[wasm_bindgen(js_name = aiInMemoryModelsStore)]
pub fn in_memory_models_store() -> ModelsStoreHandle {
	ModelsStoreHandle::new(Rc::new(pi_ai::models_store::InMemoryModelsStore::new()))
}
