//! Port of `packages/ai/src/utils/abort-signals.ts`.
//! PORT-STATUS: ported

use pi_host::abort::ListenerId;
use pi_host::{AbortController, AbortSignal};

/// The signal of `combine_abort_signals` and the cleanup that removes its listeners from the input signals.
pub struct CombinedAbortSignal {
	pub signal: Option<AbortSignal>,
	listeners: Vec<(AbortSignal, ListenerId)>,
}

impl CombinedAbortSignal {
	/// Remove the listeners added to the input signals.
	pub fn cleanup(&self) {
		for (signal, listener) in &self.listeners {
			signal.remove_listener(*listener);
		}
	}
}

pub fn combine_abort_signals(signals: &[Option<&AbortSignal>]) -> CombinedAbortSignal {
	let active_signals: Vec<&AbortSignal> = signals.iter().flatten().copied().collect();
	match active_signals.as_slice() {
		[] => {
			return CombinedAbortSignal {
				signal: None,
				listeners: Vec::new(),
			};
		}
		[signal] => {
			return CombinedAbortSignal {
				signal: Some((*signal).clone()),
				listeners: Vec::new(),
			};
		}
		_ => {}
	}

	let controller = AbortController::new();
	let mut listeners = Vec::new();
	for signal in active_signals {
		if let Some(reason) = signal.reason() {
			if !controller.signal().aborted() {
				controller.abort_with(reason);
			}
			break;
		}
		let target = controller.clone();
		let listener = signal.add_listener(move |reason| {
			if !target.signal().aborted() {
				target.abort_with(reason.clone());
			}
		});
		listeners.push((signal.clone(), listener));
	}

	CombinedAbortSignal {
		signal: Some(controller.signal()),
		listeners,
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use pi_host::PiError;

	#[test]
	fn passes_one_signal_through_and_combines_several() {
		let a = AbortController::new();
		let b = AbortController::new();
		assert!(combine_abort_signals(&[None, None]).signal.is_none());
		let single = combine_abort_signals(&[None, Some(&a.signal())]);
		a.abort_with(PiError::new("a"));
		assert!(single.signal.unwrap().aborted());

		let c = AbortController::new();
		let combined = combine_abort_signals(&[Some(&b.signal()), Some(&c.signal())]);
		c.abort_with(PiError::new("c"));
		b.abort_with(PiError::new("b"));
		assert_eq!(combined.signal.as_ref().unwrap().reason().unwrap().message, "c");

		let d = AbortController::new();
		let e = AbortController::new();
		let cleaned = combine_abort_signals(&[Some(&d.signal()), Some(&e.signal())]);
		cleaned.cleanup();
		d.abort();
		assert!(!cleaned.signal.unwrap().aborted());

		let already = combine_abort_signals(&[Some(&a.signal()), Some(&e.signal())]);
		assert_eq!(already.signal.unwrap().reason().unwrap().message, "a");
	}
}
