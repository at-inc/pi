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
	// TS checks the signal when the race starts, not when it is first awaited.
	let aborted_at_start = signal.aborted().then(|| abort_reason(signal));
	let (sender, receiver) = oneshot::channel();
	pi_host::spawn_local(async move {
		let _ = sender.send(operation.await);
	});
	let signal = signal.clone();
	async move {
		if let Some(reason) = aborted_at_start {
			return Err(reason);
		}
		let aborted = signal.aborted_future();
		futures::pin_mut!(aborted);
		// The operation first: one that settled before the abort wins, as its TS `then` ran first.
		match select(receiver, aborted).await {
			Either::Left((Ok(result), _)) => result,
			Either::Left((Err(_), _)) => futures::future::pending().await,
			Either::Right((reason, _)) => Err(reason),
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

	#[test]
	fn keeps_a_result_that_settled_before_a_late_abort() {
		pi_host::testing::run(|_| async {
			let controller = AbortController::new();
			let race = race_with_abort_signal(async { Ok(1) }, &controller.signal());
			// Yield once so the spawned operation settles, as its TS promise would before the abort.
			let mut yielded = false;
			futures::future::poll_fn(|context| {
				if yielded {
					return std::task::Poll::Ready(());
				}
				yielded = true;
				context.waker().wake_by_ref();
				std::task::Poll::Pending
			})
			.await;
			controller.abort_with(PiError::new("late abort"));
			assert_eq!(race.await.unwrap(), 1);

			let aborted = AbortController::new();
			aborted.abort_with(PiError::new("early"));
			let race = race_with_abort_signal(async { Ok(1) }, &aborted.signal());
			assert_eq!(race.await.unwrap_err().message, "early");
		});
	}
}
