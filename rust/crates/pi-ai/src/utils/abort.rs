//! Port of `packages/ai/src/utils/abort.ts`.
//! PORT-STATUS: ported
//!
//! A JS promise runs whether or not anyone awaits it, so `raceWithAbortSignal` spawns the operation with
//! `pi_host::spawn_local`: it keeps running to completion after the signal aborts, and its result is dropped.

use std::future::Future;

use futures::channel::oneshot;
use futures::future::{Either, select};
use pi_host::{AbortController, AbortSignal, PiError, PiResult};

fn abort_reason(signal: &AbortSignal) -> PiError {
	signal
		.reason()
		.unwrap_or_else(|| PiError::named("AbortError", "The operation was aborted"))
}

/// Create an operation-local signal for public APIs whose signal is optional.
pub fn operation_signal(signal: Option<&AbortSignal>) -> AbortSignal {
	signal.cloned().unwrap_or_else(|| AbortController::new().signal())
}

/// Stop waiting for an operation when its signal aborts while continuing to
/// observe the abandoned promise so a later rejection is always handled.
pub fn race_with_abort_signal<T: 'static>(
	operation: impl Future<Output = PiResult<T>> + 'static,
	signal: &AbortSignal,
) -> impl Future<Output = PiResult<T>> + 'static {
	let (sender, receiver) = oneshot::channel();
	pi_host::spawn_local(async move {
		let _ = sender.send(operation.await);
	});
	let signal = signal.clone();
	async move {
		if signal.aborted() {
			return Err(abort_reason(&signal));
		}
		let aborted = signal.aborted_future();
		futures::pin_mut!(aborted);
		match select(aborted, receiver).await {
			Either::Left((reason, _)) => Err(reason),
			Either::Right((Ok(result), _)) => result,
			Either::Right((Err(_), _)) => futures::future::pending().await,
		}
	}
}

#[cfg(test)]
mod tests {
	use std::cell::Cell;
	use std::rc::Rc;

	use super::*;

	#[test]
	fn rejects_with_the_abort_reason_and_keeps_the_operation_running() {
		let finished = Rc::new(Cell::new(false));
		let flag = finished.clone();
		pi_host::testing::run(|_| async move {
			let controller = AbortController::new();
			let (release, released) = oneshot::channel::<()>();
			let race = race_with_abort_signal(
				async move {
					let _ = released.await;
					flag.set(true);
					Ok(1)
				},
				&controller.signal(),
			);
			controller.abort_with(PiError::new("stop"));
			assert_eq!(race.await.unwrap_err().message, "stop");
			let _ = release.send(());

			let signal = operation_signal(None);
			assert_eq!(race_with_abort_signal(async { Ok(2) }, &signal).await.unwrap(), 2);
		});
		assert!(finished.get());
	}
}
