//! `AbortController` and `AbortSignal`, ported from the web platform semantics the TS code relies on.

use std::cell::RefCell;
use std::rc::Rc;

use futures::channel::oneshot;

use crate::error::{PiError, PiResult};

type Listener = Box<dyn FnOnce(&PiError)>;

struct State {
	reason: Option<PiError>,
	listeners: Vec<(u64, Listener)>,
	next_id: u64,
}

/// `AbortSignal`. Cloning shares the signal, like passing the JS object around.
#[derive(Clone)]
pub struct AbortSignal {
	state: Rc<RefCell<State>>,
}

/// Handle returned by `AbortSignal::add_listener`; pass it to `remove_listener`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ListenerId(u64);

impl AbortSignal {
	fn new() -> Self {
		Self {
			state: Rc::new(RefCell::new(State {
				reason: None,
				listeners: Vec::new(),
				next_id: 0,
			})),
		}
	}

	/// `AbortSignal.abort(reason)`: an already-aborted signal.
	pub fn aborted_with(reason: PiError) -> Self {
		let signal = Self::new();
		signal.state.borrow_mut().reason = Some(reason);
		signal
	}

	/// `signal.aborted`.
	pub fn aborted(&self) -> bool {
		self.state.borrow().reason.is_some()
	}

	/// `signal.reason`.
	pub fn reason(&self) -> Option<PiError> {
		self.state.borrow().reason.clone()
	}

	/// `signal.throwIfAborted()`.
	pub fn throw_if_aborted(&self) -> PiResult<()> {
		match self.reason() {
			Some(reason) => Err(reason),
			None => Ok(()),
		}
	}

	/// `signal.addEventListener("abort", listener, { once: true })`. A listener added to an already-aborted
	/// signal never fires, like the DOM.
	pub fn add_listener(&self, listener: impl FnOnce(&PiError) + 'static) -> ListenerId {
		let mut state = self.state.borrow_mut();
		let id = state.next_id;
		state.next_id += 1;
		if state.reason.is_none() {
			state.listeners.push((id, Box::new(listener)));
		}
		ListenerId(id)
	}

	/// `signal.removeEventListener("abort", listener)`.
	pub fn remove_listener(&self, id: ListenerId) {
		self.state
			.borrow_mut()
			.listeners
			.retain(|(listener_id, _)| *listener_id != id.0);
	}

	/// Resolves with the abort reason once the signal aborts; never resolves otherwise. Resolves immediately
	/// for an already-aborted signal.
	pub async fn aborted_future(&self) -> PiError {
		if let Some(reason) = self.reason() {
			return reason;
		}
		let (sender, receiver) = oneshot::channel();
		let id = self.add_listener(move |reason| {
			let _ = sender.send(reason.clone());
		});
		let guard = RemoveOnDrop {
			signal: self.clone(),
			id,
		};
		let reason = match receiver.await {
			Ok(reason) => reason,
			Err(_) => futures::future::pending().await,
		};
		drop(guard);
		reason
	}

	/// `AbortSignal.any(signals)`.
	pub fn any(signals: &[AbortSignal]) -> AbortSignal {
		if let Some(reason) = signals.iter().find_map(AbortSignal::reason) {
			return AbortSignal::aborted_with(reason);
		}
		let controller = AbortController::new();
		for signal in signals {
			let controller = controller.clone();
			signal.add_listener(move |reason| controller.abort_with(reason.clone()));
		}
		controller.signal()
	}

	fn abort(&self, reason: PiError) {
		let listeners = {
			let mut state = self.state.borrow_mut();
			if state.reason.is_some() {
				return;
			}
			state.reason = Some(reason.clone());
			std::mem::take(&mut state.listeners)
		};
		for (_, listener) in listeners {
			listener(&reason);
		}
	}
}

impl std::fmt::Debug for AbortSignal {
	fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
		f.debug_struct("AbortSignal").field("aborted", &self.aborted()).finish()
	}
}

struct RemoveOnDrop {
	signal: AbortSignal,
	id: ListenerId,
}

impl Drop for RemoveOnDrop {
	fn drop(&mut self) {
		self.signal.remove_listener(self.id);
	}
}

/// `AbortController`.
#[derive(Clone, Debug)]
pub struct AbortController {
	signal: AbortSignal,
}

impl AbortController {
	/// `new AbortController()`.
	pub fn new() -> Self {
		Self {
			signal: AbortSignal::new(),
		}
	}

	/// `controller.signal`.
	pub fn signal(&self) -> AbortSignal {
		self.signal.clone()
	}

	/// `controller.abort()` with the default `AbortError` reason.
	pub fn abort(&self) {
		self.signal.abort(PiError::abort());
	}

	/// `controller.abort(reason)`.
	pub fn abort_with(&self, reason: PiError) {
		self.signal.abort(reason);
	}
}

impl Default for AbortController {
	fn default() -> Self {
		Self::new()
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use std::cell::Cell;

	#[test]
	fn listeners_fire_once_and_any_follows_inputs() {
		let a = AbortController::new();
		let b = AbortController::new();
		let combined = AbortSignal::any(&[a.signal(), b.signal()]);
		let fired = Rc::new(Cell::new(0));
		let counter = fired.clone();
		combined.add_listener(move |_| counter.set(counter.get() + 1));
		b.abort_with(PiError::new("b"));
		a.abort();
		assert_eq!(fired.get(), 1);
		assert_eq!(combined.reason().unwrap().message, "b");
		assert!(combined.throw_if_aborted().is_err());
	}

	#[test]
	fn aborted_future_resolves_with_reason() {
		let controller = AbortController::new();
		let signal = controller.signal();
		controller.abort_with(PiError::new("stop"));
		let reason = futures::executor::block_on(signal.aborted_future());
		assert_eq!(reason.message, "stop");
	}
}
