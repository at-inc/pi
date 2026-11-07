//! Port of `packages/ai/test/provider-retry.test.ts`.
//!
//! `vi.useFakeTimers()` becomes [`FakeTimers`], a host whose timers fire only when the test advances the clock
//! (`vi.advanceTimersByTimeAsync`) and that counts pending timers (`vi.getTimerCount`). The tests that use real timers
//! in TS run on the same host; none of them sleeps. A provider error is a `PiError` with `status` and
//! `data.headers`.

use std::cell::{Cell, RefCell};
use std::future::Future;
use std::rc::Rc;

use futures::channel::oneshot;
use futures::executor::{LocalPool, LocalSpawner};
use futures::task::LocalSpawnExt;
use pi_ai::utils::provider_retry::{ProviderRetryOptions, retry_provider_request};
use pi_host::{AbortController, Host, HttpRequest, HttpResponse, LocalBoxFuture, PiError, PiResult};
use serde_json::{Map, Value, json};

struct Timer {
	id: u64,
	deadline: f64,
	fire: oneshot::Sender<()>,
}

/// Fake timers: `sleep` registers a timer that fires once `advance` moves the clock past its deadline.
struct FakeTimers {
	now: Cell<f64>,
	next_id: Cell<u64>,
	timers: Rc<RefCell<Vec<Timer>>>,
	spawner: LocalSpawner,
}

struct CancelOnDrop {
	id: u64,
	timers: Rc<RefCell<Vec<Timer>>>,
}

impl Drop for CancelOnDrop {
	fn drop(&mut self) {
		self.timers.borrow_mut().retain(|timer| timer.id != self.id);
	}
}

impl Host for FakeTimers {
	fn fetch(&self, _request: HttpRequest) -> LocalBoxFuture<'static, PiResult<HttpResponse>> {
		Box::pin(async { Err(PiError::type_error("fetch failed")) })
	}

	fn now_ms(&self) -> f64 {
		self.now.get()
	}

	fn monotonic_ms(&self) -> f64 {
		self.now.get()
	}

	fn env(&self, _key: &str) -> Option<String> {
		None
	}

	fn sleep(&self, ms: f64) -> LocalBoxFuture<'static, ()> {
		let ms = if (1.0..=2_147_483_647.0).contains(&ms) { ms } else { 1.0 };
		let id = self.next_id.get();
		self.next_id.set(id + 1);
		let (fire, fired) = oneshot::channel();
		self.timers.borrow_mut().push(Timer {
			id,
			deadline: self.now.get() + ms,
			fire,
		});
		let guard = CancelOnDrop {
			id,
			timers: self.timers.clone(),
		};
		Box::pin(async move {
			let _ = fired.await;
			drop(guard);
		})
	}

	fn spawn_local(&self, future: LocalBoxFuture<'static, ()>) {
		self.spawner.spawn_local(future).expect("executor is gone");
	}
}

struct Clock {
	host: Rc<FakeTimers>,
	pool: RefCell<LocalPool>,
}

impl Clock {
	fn new() -> Clock {
		let pool = LocalPool::new();
		let host = Rc::new(FakeTimers {
			now: Cell::new(0.0),
			next_id: Cell::new(0),
			timers: Rc::new(RefCell::new(Vec::new())),
			spawner: pool.spawner(),
		});
		pi_host::set_host(host.clone());
		Clock {
			host,
			pool: RefCell::new(pool),
		}
	}

	/// Starts `future` and runs it until it waits on a timer.
	fn start<T: 'static>(&self, future: impl Future<Output = T> + 'static) -> Rc<RefCell<Option<T>>> {
		let output = Rc::new(RefCell::new(None));
		let slot = output.clone();
		self.host
			.spawner
			.spawn_local(async move {
				*slot.borrow_mut() = Some(future.await);
			})
			.expect("executor is gone");
		self.pool.borrow_mut().run_until_stalled();
		output
	}

	/// `vi.advanceTimersByTimeAsync(ms)`.
	fn advance(&self, ms: f64) {
		let target = self.host.now.get() + ms;
		loop {
			let next = {
				let mut timers = self.host.timers.borrow_mut();
				let due = timers
					.iter()
					.enumerate()
					.filter(|(_, timer)| timer.deadline <= target)
					.min_by(|(_, a), (_, b)| a.deadline.total_cmp(&b.deadline))
					.map(|(index, _)| index);
				due.map(|index| timers.remove(index))
			};
			let Some(timer) = next else {
				break;
			};
			self.host.now.set(timer.deadline.max(self.host.now.get()));
			let _ = timer.fire.send(());
			self.pool.borrow_mut().run_until_stalled();
		}
		self.host.now.set(target);
		self.pool.borrow_mut().run_until_stalled();
	}

	/// `vi.getTimerCount()`.
	fn timer_count(&self) -> usize {
		self.host.timers.borrow().len()
	}

	fn run_until_stalled(&self) {
		self.pool.borrow_mut().run_until_stalled();
	}
}

fn provider_error(status: u16, headers: &[(&str, &str)]) -> PiError {
	let headers: Map<String, Value> = headers
		.iter()
		.map(|(name, value)| ((*name).to_string(), json!(value)))
		.collect();
	PiError::new(format!("Provider error: {status}"))
		.with_status(status)
		.with_data(json!({ "headers": headers }))
}

/// `vi.fn()` with `mockRejectedValueOnce(first)` and then `then` for every later call.
fn request(
	first: PiError,
	then: Result<&'static str, PiError>,
) -> (
	Rc<Cell<usize>>,
	impl FnMut() -> LocalBoxFuture<'static, PiResult<&'static str>>,
) {
	let calls = Rc::new(Cell::new(0));
	let counter = calls.clone();
	let first = RefCell::new(Some(first));
	let request = move || {
		counter.set(counter.get() + 1);
		let result = match first.borrow_mut().take() {
			Some(error) => Err(error),
			None => then.clone(),
		};
		Box::pin(async move { result }) as LocalBoxFuture<'static, PiResult<&'static str>>
	};
	(calls, request)
}

fn options(max_retries: f64) -> ProviderRetryOptions {
	ProviderRetryOptions {
		max_retries: Some(max_retries),
		..ProviderRetryOptions::default()
	}
}

mod provider_request_retries {
	use super::*;

	#[test]
	fn retries_retryable_provider_errors() {
		let clock = Clock::new();
		let (calls, request) = request(provider_error(429, &[("retry-after-ms", "1000")]), Ok("ok"));

		let result = clock.start(retry_provider_request(request, options(1.0)));
		clock.advance(999.0);
		assert_eq!(calls.get(), 1);
		clock.advance(1.0);

		assert_eq!(result.borrow_mut().take().expect("settled"), Ok("ok"));
		assert_eq!(calls.get(), 2);
	}

	#[test]
	fn does_not_retry_errors_the_provider_marks_as_non_retryable() {
		let clock = Clock::new();
		let error = provider_error(429, &[("x-should-retry", "false")]);
		let (calls, request) = request(error.clone(), Err(error.clone()));

		let result = clock.start(retry_provider_request(request, options(2.0)));
		assert_eq!(result.borrow_mut().take().expect("settled"), Err(error));
		assert_eq!(calls.get(), 1);
	}

	#[test]
	fn does_not_retry_statuses_listed_in_no_retry_statuses() {
		let clock = Clock::new();
		let error = provider_error(504, &[("retry-after-ms", "0")]);
		let (calls, request) = request(error.clone(), Err(error.clone()));

		let result = clock.start(retry_provider_request(
			request,
			ProviderRetryOptions {
				no_retry_statuses: Some(vec![504]),
				..options(2.0)
			},
		));
		assert_eq!(result.borrow_mut().take().expect("settled"), Err(error));
		assert_eq!(calls.get(), 1);
	}

	#[test]
	fn rejects_a_provider_requested_retry_delay_above_the_limit() {
		let clock = Clock::new();
		let error = provider_error(429, &[("retry-after", "277403")]);
		let (calls, request) = request(error.clone(), Err(error));

		let result = clock.start(retry_provider_request(
			request,
			ProviderRetryOptions {
				max_retry_delay_ms: Some(1000.0),
				..options(1.0)
			},
		));
		let error = result.borrow_mut().take().expect("settled").expect_err("rejects");
		assert!(
			error.message.contains("Server requested 277403s retry delay (max: 1s)"),
			"{}",
			error.message
		);
		assert_eq!(calls.get(), 1);
	}

	#[test]
	fn allows_disabling_the_provider_requested_retry_delay_cap() {
		let clock = Clock::new();
		let (calls, request) = request(provider_error(429, &[("retry-after", "2")]), Ok("ok"));

		let result = clock.start(retry_provider_request(
			request,
			ProviderRetryOptions {
				max_retry_delay_ms: Some(0.0),
				..options(1.0)
			},
		));
		clock.advance(1999.0);
		assert_eq!(calls.get(), 1);
		clock.advance(1.0);

		assert_eq!(result.borrow_mut().take().expect("settled"), Ok("ok"));
		assert_eq!(calls.get(), 2);
	}

	#[test]
	fn aborts_a_provider_requested_retry_delay() {
		let clock = Clock::new();
		let controller = AbortController::new();
		let error = provider_error(429, &[("retry-after", "277403")]);
		let (calls, request) = request(error.clone(), Err(error));

		let result = clock.start(retry_provider_request(
			request,
			ProviderRetryOptions {
				max_retry_delay_ms: Some(0.0),
				signal: Some(controller.signal()),
				..options(2.0)
			},
		));
		clock.advance(0.0);
		assert_eq!(calls.get(), 1);
		assert_eq!(clock.timer_count(), 1);

		controller.abort();
		clock.run_until_stalled();

		let error = result.borrow_mut().take().expect("settled").expect_err("rejects");
		assert_eq!(error.name, "AbortError");
		assert_eq!(calls.get(), 1);
		assert_eq!(clock.timer_count(), 0);
	}
}
