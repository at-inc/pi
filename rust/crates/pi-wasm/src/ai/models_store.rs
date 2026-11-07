//! Exports of `models-store`: the Rust `InMemoryModelsStore` behind the facade class.

use std::rc::Rc;

use js_sys::{Promise, Reflect};
use pi_ai::{InMemoryModelsStore, ModelsStore, ModelsStoreEntry, ModelsStoreOperationOptions};
use wasm_bindgen::prelude::*;

use crate::options::arg;

/// A Rust `InMemoryModelsStore`, owned by the facade's `InMemoryModelsStore` object.
#[wasm_bindgen]
pub struct ModelsStoreHandle {
	inner: Rc<InMemoryModelsStore>,
}

/// `new InMemoryModelsStore()`.
#[wasm_bindgen(js_name = aiInMemoryModelsStore)]
pub fn in_memory_models_store() -> ModelsStoreHandle {
	ModelsStoreHandle {
		inner: Rc::new(InMemoryModelsStore::new()),
	}
}

#[wasm_bindgen]
impl ModelsStoreHandle {
	/// `store.read(providerId, options)`.
	pub fn read(&self, provider_id: &str, options: JsValue) -> Promise {
		let pending = self.inner.read(provider_id, store_options(&options));
		future(async move {
			match pending.await? {
				Some(entry) => crate::json::to_js(&entry),
				None => Ok(JsValue::UNDEFINED),
			}
		})
	}

	/// `store.write(providerId, entry, options)`.
	pub fn write(&self, provider_id: &str, entry: JsValue, options: JsValue) -> Result<Promise, JsValue> {
		let entry: ModelsStoreEntry = arg(entry)?;
		let pending = self.inner.write(provider_id, entry, store_options(&options));
		Ok(future(async move {
			pending.await?;
			Ok(JsValue::UNDEFINED)
		}))
	}

	/// `store.delete(providerId, options)`.
	pub fn delete(&self, provider_id: &str, options: JsValue) -> Promise {
		let pending = self.inner.delete(provider_id, store_options(&options));
		future(async move {
			pending.await?;
			Ok(JsValue::UNDEFINED)
		})
	}
}

/// `{ signal }` of a JS options object.
fn store_options(options: &JsValue) -> Option<ModelsStoreOperationOptions> {
	let signal = Reflect::get(options, &"signal".into()).unwrap_or(JsValue::UNDEFINED);
	options.is_object().then(|| ModelsStoreOperationOptions {
		signal: crate::abort::from_js(&signal),
	})
}

fn future(future: impl std::future::Future<Output = pi_host::PiResult<JsValue>> + 'static) -> Promise {
	wasm_bindgen_futures::future_to_promise(async move { future.await.map_err(|error| crate::error::to_js(&error)) })
}
