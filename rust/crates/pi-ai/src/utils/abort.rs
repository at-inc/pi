//! Port of `packages/ai/src/utils/abort.ts`.
//! PORT-STATUS: reviewed
//!
//! A JS promise runs whether or not anyone awaits it, so `raceWithAbortSignal` spawns the operation with
//! `pi_host::spawn_local`: it keeps running to completion after the signal aborts, and its result is dropped.
//! A JS async operation runs its synchronous prefix when it is called; the spawned one first runs when the executor
//! polls its task. If the host drops that task unfinished, the race settles only when the signal aborts, as TS
//! rejects on abort whatever the operation does.

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
pub fn race_with_abort_signal<T: 'static, F: Future<Output = PiResult<T>> + 'static>(
	operation: F,
	signal: &AbortSignal,
) -> impl Future<Output = PiResult<T>> + 'static + use<T, F> {
	// TS checks the signal when the race starts, not when it is first awaited.
	let aborted_at_start = signal.aborted().then(|| abort_reason(signal));
	let (sender, receiver) = oneshot::channel();
	let settle_signal = signal.clone();
	pi_host::spawn_local(async move {
		let result = operation.await;
		// TS's abort listener rejects the race at once, so an abort that lands while the operation runs wins.
		let _ = sender.send((!settle_signal.aborted()).then_some(result));
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
			Either::Left((Ok(Some(result)), _)) => result,
			Either::Left((Ok(None), _)) => Err(abort_reason(&signal)),
			Either::Left((Err(_), aborted)) => Err(aborted.await),
			Either::Right((reason, _)) => Err(reason),
		}
	}
}

#[cfg(test)]
mod tests {
	use std::cell::Cell;
	use std::rc::Rc;

	use pi_host::{Host, HttpRequest, HttpResponse, LocalBoxFuture};

	use super::*;

	/// A host whose `spawn_local` drops the task unpolled, as a host shutting down would.
	struct DroppingHost(Rc<dyn Host>);

	impl Host for DroppingHost {
		fn fetch(&self, request: HttpRequest) -> LocalBoxFuture<'static, PiResult<HttpResponse>> {
			self.0.fetch(request)
		}

		fn now_ms(&self) -> f64 {
			self.0.now_ms()
		}

		fn monotonic_ms(&self) -> f64 {
			self.0.monotonic_ms()
		}

		fn env(&self, key: &str) -> Option<String> {
			self.0.env(key)
		}

		fn sleep(&self, ms: f64) -> LocalBoxFuture<'static, ()> {
			self.0.sleep(ms)
		}

		fn spawn_local(&self, _future: LocalBoxFuture<'static, ()>) {}
	}

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

	#[test]
	fn rejects_when_the_signal_aborts_before_the_operation_settles() {
		pi_host::testing::run(|_| async {
			let controller = AbortController::new();
			let aborter = controller.clone();
			let race = race_with_abort_signal(
				async move {
					aborter.abort_with(PiError::new("aborted while running"));
					Ok(1)
				},
				&controller.signal(),
			);
			assert_eq!(race.await.unwrap_err().message, "aborted while running");
		});
	}

	#[test]
	fn rejects_on_abort_when_the_host_drops_the_operation_task() {
		pi_host::testing::run(|_| async {
			let controller = AbortController::new();
			let host = pi_host::current();
			pi_host::set_host(Rc::new(DroppingHost(host.clone())));
			let race = race_with_abort_signal(async { Ok(1) }, &controller.signal());
			pi_host::set_host(host);
			let aborter = controller.clone();
			pi_host::spawn_local(async move { aborter.abort_with(PiError::new("stop")) });
			assert_eq!(race.await.unwrap_err().message, "stop");
		});
	}
}
