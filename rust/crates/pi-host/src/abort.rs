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

	/// `a === b`: whether both handles are the same signal.
	pub fn ptr_eq(a: &AbortSignal, b: &AbortSignal) -> bool {
		Rc::ptr_eq(&a.state, &b.state)
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

	/// `AbortSignal.timeout(ms)`: a signal that aborts with a `TimeoutError` after `ms` milliseconds of host time.
	pub fn timeout(ms: f64) -> AbortSignal {
		let controller = AbortController::new();
		let signal = controller.signal();
		crate::host::spawn_local(async move {
			crate::host::current().sleep(ms).await;
			controller.abort_with(PiError::named(
				"TimeoutError",
				"The operation was aborted due to timeout",
			));
		});
		signal
	}

	/// `AbortSignal.timeout(ms)` with Node's `validateUint32(delay)` check: a `RangeError` (`ERR_OUT_OF_RANGE`) for a
	/// delay that is not an integer in `0..=4294967295`, else [`AbortSignal::timeout`].
	pub fn try_timeout(ms: f64) -> PiResult<AbortSignal> {
		let range = if ms.fract() != 0.0 || !ms.is_finite() {
			"an integer"
		} else if !(0.0..=4_294_967_295.0).contains(&ms) {
			">= 0 && <= 4294967295"
		} else {
			return Ok(Self::timeout(ms));
		};
		let text = crate::js::number_to_string(ms);
		let received = if ms.fract() == 0.0 && ms.abs() > 4_294_967_296.0 {
			let start = usize::from(text.starts_with('-'));
			let mut end = text.len();
			let mut grouped = String::new();
			while end >= start + 4 {
				grouped = format!("_{}{grouped}", &text[end - 3..end]);
				end -= 3;
			}
			format!("{}{grouped}", &text[..end])
		} else {
			text
		};
		Err(PiError::named(
			"RangeError",
			format!("The value of \"delay\" is out of range. It must be {range}. Received {received}"),
		)
		.with_data(serde_json::json!({ "code": "ERR_OUT_OF_RANGE" })))
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
	fn try_timeout_rejects_delays_node_rejects() {
		let message = |ms: f64| {
			AbortSignal::try_timeout(ms)
				.err()
				.map(|error| (error.name, error.message))
		};
		let range = |rule: &str, received: &str| {
			Some((
				"RangeError".to_string(),
				format!("The value of \"delay\" is out of range. It must be {rule}. Received {received}"),
			))
		};
		assert_eq!(message(1.5), range("an integer", "1.5"));
		assert_eq!(message(f64::NAN), range("an integer", "NaN"));
		assert_eq!(message(f64::NEG_INFINITY), range("an integer", "-Infinity"));
		assert_eq!(message(-1.0), range(">= 0 && <= 4294967295", "-1"));
		assert_eq!(message(4_294_967_296.0), range(">= 0 && <= 4294967295", "4294967296"));
		assert_eq!(message(-1e10), range(">= 0 && <= 4294967295", "-10_000_000_000"));
		assert_eq!(message(1e21), range(">= 0 && <= 4294967295", "1e_+21"));
	}

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
