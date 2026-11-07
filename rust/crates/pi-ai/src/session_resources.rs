//! Port of `packages/ai/src/session-resources.ts`.
//! PORT-STATUS: reviewed
//!
//! The `Set` of cleanups is a per-thread list that keeps insertion order and skips a cleanup that is already in it
//! (`Rc` identity). Like `Set` iteration, a cleanup pass sees cleanups registered during it and skips the ones
//! unregistered before their turn: removals leave a hole until no pass is running. A cleanup that throws returns
//! `Err`. The `AggregateError` is a `PiError` named `AggregateError` whose `data.errors` lists each error as
//! `{ name, message, status?, data? }`.

use std::cell::RefCell;
use std::rc::Rc;

use pi_host::{PiError, PiResult};
use serde_json::{Value, json};

pub type SessionResourceCleanup = Rc<dyn Fn(Option<&str>) -> PiResult<()>>;

#[derive(Default)]
struct Cleanups {
	entries: Vec<Option<SessionResourceCleanup>>,
	running_passes: usize,
}

thread_local! {
	static SESSION_RESOURCE_CLEANUPS: RefCell<Cleanups> = RefCell::default();
}

/// Registers `cleanup` and returns the function that unregisters it.
pub fn register_session_resource_cleanup(cleanup: SessionResourceCleanup) -> Box<dyn Fn()> {
	SESSION_RESOURCE_CLEANUPS.with(|cleanups| {
		let mut cleanups = cleanups.borrow_mut();
		if !cleanups
			.entries
			.iter()
			.flatten()
			.any(|entry| Rc::ptr_eq(entry, &cleanup))
		{
			cleanups.entries.push(Some(cleanup.clone()));
		}
	});
	Box::new(move || {
		SESSION_RESOURCE_CLEANUPS.with(|cleanups| {
			let mut cleanups = cleanups.borrow_mut();
			if let Some(slot) = cleanups
				.entries
				.iter_mut()
				.find(|entry| entry.as_ref().is_some_and(|entry| Rc::ptr_eq(entry, &cleanup)))
			{
				*slot = None;
			}
			if cleanups.running_passes == 0 {
				cleanups.entries.retain(Option::is_some);
			}
		});
	})
}

pub fn cleanup_session_resources(session_id: Option<&str>) -> PiResult<()> {
	SESSION_RESOURCE_CLEANUPS.with(|cleanups| cleanups.borrow_mut().running_passes += 1);
	let mut errors: Vec<Value> = Vec::new();
	let mut index = 0;
	// The list is read again at every step, so cleanups may register and unregister others while the pass runs.
	while let Some(entry) = SESSION_RESOURCE_CLEANUPS.with(|cleanups| cleanups.borrow().entries.get(index).cloned()) {
		index += 1;
		let Some(cleanup) = entry else {
			continue;
		};
		if let Err(error) = cleanup(session_id) {
			let mut entry = json!({ "name": error.name, "message": error.message });
			if let Some(status) = error.status {
				entry["status"] = json!(status);
			}
			if let Some(data) = error.data {
				entry["data"] = data;
			}
			errors.push(entry);
		}
	}
	SESSION_RESOURCE_CLEANUPS.with(|cleanups| {
		let mut cleanups = cleanups.borrow_mut();
		cleanups.running_passes -= 1;
		if cleanups.running_passes == 0 {
			cleanups.entries.retain(Option::is_some);
		}
	});
	if !errors.is_empty() {
		return Err(PiError::named("AggregateError", "Failed to cleanup session resources")
			.with_data(json!({ "errors": errors })));
	}
	Ok(())
}

#[cfg(test)]
mod tests {
	use std::cell::RefCell;

	use super::*;

	#[test]
	fn a_pass_follows_registrations_and_removals_made_during_it() {
		let calls: Rc<RefCell<Vec<&'static str>>> = Rc::default();
		let record = |name: &'static str| -> SessionResourceCleanup {
			let calls = calls.clone();
			Rc::new(move |_| {
				calls.borrow_mut().push(name);
				Ok(())
			})
		};
		type Unregister = Box<dyn Fn()>;
		let unregister_b: Rc<RefCell<Option<Unregister>>> = Rc::default();
		let a: SessionResourceCleanup = {
			let (calls, unregister_b, c) = (calls.clone(), unregister_b.clone(), record("c"));
			Rc::new(move |_| {
				calls.borrow_mut().push("a");
				if let Some(unregister) = unregister_b.borrow_mut().take() {
					unregister();
				}
				let _unregister_c = register_session_resource_cleanup(c.clone());
				Ok(())
			})
		};
		let unregister_a = register_session_resource_cleanup(a);
		*unregister_b.borrow_mut() = Some(register_session_resource_cleanup(record("b")));
		cleanup_session_resources(None).unwrap();
		assert_eq!(*calls.borrow(), ["a", "c"]);
		unregister_a();
	}

	#[test]
	fn keeps_status_and_data_of_failed_cleanups() {
		let unregister = register_session_resource_cleanup(Rc::new(|_| {
			let mut error = PiError::new("boom");
			error.status = Some(429);
			error.data = Some(json!({ "code": "X" }));
			Err(error)
		}));
		let error = cleanup_session_resources(None).unwrap_err();
		unregister();
		assert_eq!(error.name, "AggregateError");
		assert_eq!(
			error.data,
			Some(json!({ "errors": [{ "name": "Error", "message": "boom", "status": 429, "data": { "code": "X" } }] }))
		);
	}
}
