//! Exports of `session-resources`: JS cleanups join the core's registry, so `cleanupSessionResources` runs them
//! together with the core's own (the Codex WebSocket sessions). A JS function registered twice is one entry, as in
//! the TS `Set`.

use std::cell::RefCell;
use std::rc::Rc;

use js_sys::{Function, Object};
use pi_ai::session_resources::{self, SessionResourceCleanup};
use wasm_bindgen::prelude::*;

thread_local! {
	/// The core cleanup of each registered JS function, so registering it again finds the same entry.
	static REGISTERED: RefCell<Vec<(Function, SessionResourceCleanup)>> = const { RefCell::new(Vec::new()) };
}

/// `registerSessionResourceCleanup(cleanup)`: returns the function that unregisters it.
#[wasm_bindgen(js_name = aiRegisterSessionResourceCleanup)]
pub fn register_session_resource_cleanup(cleanup: Function) -> JsValue {
	let core = REGISTERED.with(|registered| {
		let mut registered = registered.borrow_mut();
		if let Some((_, core)) = registered.iter().find(|(function, _)| Object::is(function, &cleanup)) {
			return core.clone();
		}
		let function = cleanup.clone();
		let core: SessionResourceCleanup = Rc::new(move |session_id: Option<&str>| {
			let session_id = session_id.map_or(JsValue::UNDEFINED, JsValue::from);
			function
				.call1(&JsValue::UNDEFINED, &session_id)
				.map(|_| ())
				.map_err(crate::error::from_js)
		});
		registered.push((cleanup.clone(), core.clone()));
		core
	});
	let unregister = session_resources::register_session_resource_cleanup(core.clone());
	Closure::<dyn Fn()>::new(move || {
		unregister();
		REGISTERED.with(|registered| registered.borrow_mut().retain(|(_, entry)| !Rc::ptr_eq(entry, &core)));
	})
	.into_js_value()
}

/// `cleanupSessionResources(sessionId)`: throws a `PiError` named `AggregateError` with `errors` when cleanups threw.
#[wasm_bindgen(js_name = aiCleanupSessionResources)]
pub fn cleanup_session_resources(session_id: Option<String>) -> Result<(), JsValue> {
	session_resources::cleanup_session_resources(session_id.as_deref()).map_err(|error| crate::error::to_js(&error))
}
