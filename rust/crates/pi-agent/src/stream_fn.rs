//! Port of `packages/agent/src/stream-fn.ts`.
//! PORT-STATUS: ported
//!
//! The module-level `defaultStreamFn` is thread-local: the core is single-threaded, and each thread is one JS realm.

use std::cell::RefCell;

use pi_host::{PiError, PiResult};

use crate::types::StreamFn;

thread_local! {
	static DEFAULT_STREAM_FN: RefCell<Option<StreamFn>> = const { RefCell::new(None) };
}

/// Configure the fallback used by Agent and low-level loops when callers omit streamFn.
///
/// Hosts that provide a default model runtime can install its stream function here
/// without making pi-agent-core depend on a provider catalog or compatibility layer.
pub fn set_default_stream_fn(stream_fn: Option<StreamFn>) {
	DEFAULT_STREAM_FN.with(|slot| *slot.borrow_mut() = stream_fn);
}

/// The configured fallback stream function. Throws when none is configured.
pub fn get_default_stream_fn() -> PiResult<StreamFn> {
	DEFAULT_STREAM_FN.with(|slot| slot.borrow().clone()).ok_or_else(|| {
		PiError::new("No default stream function configured. Pass streamFn explicitly or call setDefaultStreamFn().")
	})
}
