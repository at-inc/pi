//! Port of `packages/ai/src/auth/credential-store.ts`.
//! PORT-STATUS: ported
//!
//! The TS promise chain per provider is a `Shared` future that settles when the queued task settles; each entry
//! carries a sequence number that stands in for the identity check `this.chains.get(providerId) === tail`.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;

use futures::FutureExt;
use futures::channel::oneshot;
use futures::future::Shared;
use indexmap::IndexMap;
use pi_host::{LocalBoxFuture, PiResult};

use crate::auth::types::{AuthOperationOptions, Credential, CredentialInfo, CredentialModifier, CredentialStore};
use crate::utils::abort::{operation_signal, race_with_abort_signal};

type Chain = Shared<LocalBoxFuture<'static, ()>>;

/// Default in-memory credential store. Apps inject persistent stores.
/// Keyed by `Provider.id`, one credential per provider; see `CredentialStore`.
/// Writes are serialized per provider through a promise chain.
#[derive(Default)]
pub struct InMemoryCredentialStore {
	credentials: Rc<RefCell<IndexMap<String, Credential>>>,
	chains: Rc<RefCell<HashMap<String, (u64, Chain)>>>,
	next_chain_id: Cell<u64>,
}

impl InMemoryCredentialStore {
	/// `new InMemoryCredentialStore()`.
	pub fn new() -> Self {
		Self::default()
	}

	/// Serialize tasks per provider id without releasing the chain before active work settles.
	fn enqueue<T: 'static>(
		&self,
		provider_id: &str,
		task: impl FnOnce() -> LocalBoxFuture<'static, PiResult<T>> + 'static,
		options: Option<AuthOperationOptions>,
	) -> LocalBoxFuture<'static, PiResult<T>> {
		let signal = operation_signal(options.as_ref().and_then(|options| options.signal.as_ref()));
		let previous = self.chains.borrow().get(provider_id).map(|(_, chain)| chain.clone());
		let (settled, on_settled) = oneshot::channel::<()>();
		let queued = {
			let signal = signal.clone();
			async move {
				if let Some(previous) = previous {
					previous.await;
				}
				let result = match signal.throw_if_aborted() {
					Ok(()) => task().await,
					Err(error) => Err(error),
				};
				let _ = settled.send(());
				result
			}
		};
		let tail: Chain = on_settled.map(|_| ()).boxed_local().shared();
		let id = self.next_chain_id.get();
		self.next_chain_id.set(id + 1);
		self.chains
			.borrow_mut()
			.insert(provider_id.to_string(), (id, tail.clone()));
		let chains = self.chains.clone();
		let provider_id = provider_id.to_string();
		pi_host::spawn_local(async move {
			tail.await;
			let mut chains = chains.borrow_mut();
			if chains.get(&provider_id).is_some_and(|(current, _)| *current == id) {
				chains.remove(&provider_id);
			}
		});
		Box::pin(race_with_abort_signal(queued, &signal))
	}
}

impl CredentialStore for InMemoryCredentialStore {
	fn read(
		&self,
		provider_id: &str,
		options: Option<AuthOperationOptions>,
	) -> LocalBoxFuture<'static, PiResult<Option<Credential>>> {
		let result = match options.and_then(|options| options.signal) {
			Some(signal) => signal.throw_if_aborted(),
			None => Ok(()),
		}
		.map(|()| self.credentials.borrow().get(provider_id).cloned());
		Box::pin(async move { result })
	}

	fn list(&self, options: Option<AuthOperationOptions>) -> LocalBoxFuture<'static, PiResult<Vec<CredentialInfo>>> {
		let result = match options.and_then(|options| options.signal) {
			Some(signal) => signal.throw_if_aborted(),
			None => Ok(()),
		}
		.map(|()| {
			self.credentials
				.borrow()
				.iter()
				.map(|(provider_id, credential)| CredentialInfo {
					provider_id: provider_id.clone(),
					r#type: credential.r#type(),
				})
				.collect()
		});
		Box::pin(async move { result })
	}

	fn modify(
		&self,
		provider_id: &str,
		r#fn: CredentialModifier,
		options: Option<AuthOperationOptions>,
	) -> LocalBoxFuture<'static, PiResult<Option<Credential>>> {
		let credentials = self.credentials.clone();
		let key = provider_id.to_string();
		let signal = options.as_ref().and_then(|options| options.signal.clone());
		self.enqueue(
			provider_id,
			move || {
				Box::pin(async move {
					let current = credentials.borrow().get(&key).cloned();
					let next = r#fn(current.clone()).await?;
					if let Some(signal) = &signal {
						signal.throw_if_aborted()?;
					}
					if let Some(next) = &next {
						credentials.borrow_mut().insert(key, next.clone());
					}
					Ok(next.or(current))
				})
			},
			options,
		)
	}

	fn delete(
		&self,
		provider_id: &str,
		options: Option<AuthOperationOptions>,
	) -> LocalBoxFuture<'static, PiResult<()>> {
		let credentials = self.credentials.clone();
		let key = provider_id.to_string();
		self.enqueue(
			provider_id,
			move || {
				Box::pin(async move {
					credentials.borrow_mut().shift_remove(&key);
					Ok(())
				})
			},
			options,
		)
	}
}
