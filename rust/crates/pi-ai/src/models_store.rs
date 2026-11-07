//! Port of `packages/ai/src/models-store.ts`.
//! PORT-STATUS: ported
//!
//! `structuredClone` of an entry is a Rust `clone`.

use std::cell::RefCell;
use std::rc::Rc;

use indexmap::IndexMap;
use pi_host::{AbortSignal, LocalBoxFuture, PiResult};
use serde::{Deserialize, Serialize};

use crate::types::AnyModel;

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelsStoreEntry {
	/// Persisted models of every type.
	pub models: Vec<AnyModel>,
	/// Unix timestamp from the remote catalog's Last-Modified header.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub last_modified: Option<f64>,
	/// Unix timestamp of the last completed remote check.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub checked_at: Option<f64>,
	/// Opaque validator from the remote catalog's ETag header, stored verbatim
	/// (quotes included) and echoed back as If-None-Match.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub etag: Option<String>,
}

#[derive(Clone, Debug, Default)]
pub struct ModelsStoreOperationOptions {
	pub signal: Option<AbortSignal>,
}

/// Persistent model catalogs keyed by provider ID.
pub trait ModelsStore {
	fn read(
		&self,
		provider_id: &str,
		options: Option<ModelsStoreOperationOptions>,
	) -> LocalBoxFuture<'static, PiResult<Option<ModelsStoreEntry>>>;
	fn write(
		&self,
		provider_id: &str,
		entry: ModelsStoreEntry,
		options: Option<ModelsStoreOperationOptions>,
	) -> LocalBoxFuture<'static, PiResult<()>>;
	fn delete(
		&self,
		provider_id: &str,
		options: Option<ModelsStoreOperationOptions>,
	) -> LocalBoxFuture<'static, PiResult<()>>;
}

#[derive(Default)]
pub struct InMemoryModelsStore {
	entries: Rc<RefCell<IndexMap<String, ModelsStoreEntry>>>,
}

impl InMemoryModelsStore {
	/// `new InMemoryModelsStore()`.
	pub fn new() -> Self {
		Self::default()
	}
}

fn throw_if_aborted(options: &Option<ModelsStoreOperationOptions>) -> PiResult<()> {
	match options.as_ref().and_then(|options| options.signal.as_ref()) {
		Some(signal) => signal.throw_if_aborted(),
		None => Ok(()),
	}
}

impl ModelsStore for InMemoryModelsStore {
	fn read(
		&self,
		provider_id: &str,
		options: Option<ModelsStoreOperationOptions>,
	) -> LocalBoxFuture<'static, PiResult<Option<ModelsStoreEntry>>> {
		let result = throw_if_aborted(&options).map(|()| self.entries.borrow().get(provider_id).cloned());
		Box::pin(async move { result })
	}

	fn write(
		&self,
		provider_id: &str,
		entry: ModelsStoreEntry,
		options: Option<ModelsStoreOperationOptions>,
	) -> LocalBoxFuture<'static, PiResult<()>> {
		let result = throw_if_aborted(&options).map(|()| {
			self.entries.borrow_mut().insert(provider_id.to_string(), entry);
		});
		Box::pin(async move { result })
	}

	fn delete(
		&self,
		provider_id: &str,
		options: Option<ModelsStoreOperationOptions>,
	) -> LocalBoxFuture<'static, PiResult<()>> {
		let result = throw_if_aborted(&options).map(|()| {
			self.entries.borrow_mut().shift_remove(provider_id);
		});
		Box::pin(async move { result })
	}
}
