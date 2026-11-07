//! Port of `packages/ai/src/session-resources.ts`.
//! PORT-STATUS: ported
//!
//! The `Set` of cleanups is a per-thread list that keeps insertion order and skips a cleanup that is already in it
//! (`Rc` identity). A cleanup that throws returns `Err`. The `AggregateError` is a `PiError` named
//! `AggregateError` whose `data.errors` lists each error's `{ name, message }`.

use std::cell::RefCell;
use std::rc::Rc;

use pi_host::{PiError, PiResult};
use serde_json::{Value, json};

pub type SessionResourceCleanup = Rc<dyn Fn(Option<&str>) -> PiResult<()>>;

thread_local! {
	static SESSION_RESOURCE_CLEANUPS: RefCell<Vec<SessionResourceCleanup>> = const { RefCell::new(Vec::new()) };
}

/// Registers `cleanup` and returns the function that unregisters it.
pub fn register_session_resource_cleanup(cleanup: SessionResourceCleanup) -> Box<dyn Fn()> {
	SESSION_RESOURCE_CLEANUPS.with(|cleanups| {
		let mut cleanups = cleanups.borrow_mut();
		if !cleanups.iter().any(|entry| Rc::ptr_eq(entry, &cleanup)) {
			cleanups.push(cleanup.clone());
		}
	});
	Box::new(move || {
		SESSION_RESOURCE_CLEANUPS.with(|cleanups| cleanups.borrow_mut().retain(|entry| !Rc::ptr_eq(entry, &cleanup)));
	})
}

pub fn cleanup_session_resources(session_id: Option<&str>) -> PiResult<()> {
	let cleanups = SESSION_RESOURCE_CLEANUPS.with(|cleanups| cleanups.borrow().clone());
	let mut errors: Vec<Value> = Vec::new();
	for cleanup in cleanups {
		if let Err(error) = cleanup(session_id) {
			errors.push(json!({ "name": error.name, "message": error.message }));
		}
	}
	if !errors.is_empty() {
		return Err(PiError::named("AggregateError", "Failed to cleanup session resources")
			.with_data(json!({ "errors": errors })));
	}
	Ok(())
}
