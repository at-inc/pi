//! Scripted host for tests: a programmable `fetch`, a virtual clock, an in-memory environment, and a
//! single-threaded executor. Ported vitest suites that stub `globalThis.fetch` or use fake timers use this.
//!
//! Timers are fake, like vitest's `vi.useFakeTimers()` with `advanceTimersByTimeAsync` run to completion: a
//! `sleep` registers a deadline on the virtual clock, and only when every task is stalled does the clock jump to
//! the earliest pending deadline and wake that timer. Concurrent timers (a request timeout racing a poll interval)
//! therefore fire in deadline order without adding up, and a dropped sleep never moves the clock.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::rc::{Rc, Weak};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::task::{Context, Poll, Waker};

use futures::executor::LocalPool;
use futures::task::{ArcWake, LocalSpawnExt};

use crate::LocalBoxFuture;
use crate::error::{PiError, PiResult};
use crate::host::{Host, set_host};
use crate::http::{HttpRequest, HttpResponse};

type Handler = Rc<dyn Fn(HttpRequest) -> LocalBoxFuture<'static, PiResult<HttpResponse>>>;

/// The test host. Sleeping waits for the virtual clock to reach the deadline, which happens as soon as nothing else
/// can run, so retry and backoff logic runs instantly while `now_ms` still moves forward.
pub struct TestHost {
	handler: RefCell<Option<Handler>>,
	env: RefCell<HashMap<String, String>>,
	clock_ms: RefCell<f64>,
	requests: RefCell<Vec<RecordedRequest>>,
	timers: RefCell<Vec<Weak<Timer>>>,
	spawner: futures::executor::LocalSpawner,
}

struct Timer {
	deadline: f64,
	fired: Cell<bool>,
	waker: RefCell<Option<Waker>>,
}

struct Sleep {
	timer: Rc<Timer>,
}

impl Future for Sleep {
	type Output = ();

	fn poll(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<()> {
		if self.timer.fired.get() {
			return Poll::Ready(());
		}
		*self.timer.waker.borrow_mut() = Some(context.waker().clone());
		Poll::Pending
	}
}

/// A request `fetch` received, with its body decoded as UTF-8.
#[derive(Clone, Debug)]
pub struct RecordedRequest {
	pub method: String,
	pub url: String,
	pub headers: indexmap::IndexMap<String, String>,
	pub body: Option<String>,
}

impl RecordedRequest {
	/// The body parsed as JSON.
	pub fn json(&self) -> serde_json::Value {
		serde_json::from_str(self.body.as_deref().unwrap_or("null")).expect("request body is not JSON")
	}

	/// Case-insensitive header lookup.
	pub fn header(&self, name: &str) -> Option<&str> {
		self.headers
			.iter()
			.find(|(key, _)| key.eq_ignore_ascii_case(name))
			.map(|(_, value)| value.as_str())
	}
}

impl TestHost {
	/// Replaces the fetch handler.
	pub fn on_fetch(&self, handler: impl Fn(HttpRequest) -> LocalBoxFuture<'static, PiResult<HttpResponse>> + 'static) {
		*self.handler.borrow_mut() = Some(Rc::new(handler));
	}

	/// Serves every request with the same status and body.
	pub fn respond_with(&self, status: u16, headers: &[(&str, &str)], body: &'static str) {
		let headers: indexmap::IndexMap<String, String> = headers
			.iter()
			.map(|(key, value)| ((*key).to_string(), (*value).to_string()))
			.collect();
		self.on_fetch(move |_| {
			let headers = headers.clone();
			Box::pin(async move { Ok(HttpResponse::from_bytes(status, headers, body)) })
		});
	}

	/// Sets `process.env[key]`.
	pub fn set_env(&self, key: &str, value: &str) {
		self.env.borrow_mut().insert(key.into(), value.into());
	}

	/// Every request `fetch` received so far.
	pub fn requests(&self) -> Vec<RecordedRequest> {
		self.requests.borrow().clone()
	}

	/// Moves the virtual clock forward. Timers whose deadline passed fire once every task is stalled.
	pub fn advance(&self, ms: f64) {
		*self.clock_ms.borrow_mut() += ms;
	}

	/// Moves the clock to the earliest live timer and fires it; timers with equal deadlines fire one per call in
	/// the order they were created, like `setTimeout`. Returns false when no timer is pending.
	fn fire_next_timer(&self) -> bool {
		let next = {
			let mut timers = self.timers.borrow_mut();
			timers.retain(|timer| timer.upgrade().is_some_and(|timer| !timer.fired.get()));
			let Some(index) = timers
				.iter()
				.enumerate()
				.filter_map(|(index, timer)| timer.upgrade().map(|timer| (index, timer.deadline)))
				.min_by(|(_, a), (_, b)| a.total_cmp(b))
				.map(|(index, _)| index)
			else {
				return false;
			};
			timers.remove(index).upgrade()
		};
		let Some(timer) = next else {
			return true;
		};
		{
			let mut clock = self.clock_ms.borrow_mut();
			*clock = clock.max(timer.deadline);
		}
		timer.fired.set(true);
		let waker = timer.waker.borrow_mut().take();
		if let Some(waker) = waker {
			waker.wake();
		}
		true
	}
}

impl Host for TestHost {
	fn fetch(&self, request: HttpRequest) -> LocalBoxFuture<'static, PiResult<HttpResponse>> {
		self.requests.borrow_mut().push(RecordedRequest {
			method: request.method.clone(),
			url: request.url.clone(),
			headers: request.headers.clone(),
			body: request
				.body
				.as_ref()
				.map(|body| String::from_utf8_lossy(body).into_owned()),
		});
		let handler = self.handler.borrow().clone();
		match handler {
			Some(handler) => handler(request),
			None => Box::pin(async { Err(PiError::type_error("fetch failed: no TestHost handler")) }),
		}
	}

	fn now_ms(&self) -> f64 {
		self.clock_ms.borrow().floor()
	}

	fn monotonic_ms(&self) -> f64 {
		*self.clock_ms.borrow()
	}

	fn env(&self, key: &str) -> Option<String> {
		self.env.borrow().get(key).cloned()
	}

	fn sleep(&self, ms: f64) -> LocalBoxFuture<'static, ()> {
		let timer = Rc::new(Timer {
			deadline: *self.clock_ms.borrow() + ms.max(0.0),
			fired: Cell::new(false),
			waker: RefCell::new(None),
		});
		self.timers.borrow_mut().push(Rc::downgrade(&timer));
		Box::pin(Sleep { timer })
	}

	fn spawn_local(&self, future: LocalBoxFuture<'static, ()>) {
		self.spawner.spawn_local(future).expect("TestHost executor is gone");
	}
}

struct MainWake {
	woken: AtomicBool,
	thread: std::thread::Thread,
}

impl ArcWake for MainWake {
	fn wake_by_ref(arc_self: &Arc<Self>) {
		arc_self.woken.store(true, Ordering::SeqCst);
		arc_self.thread.unpark();
	}
}

/// Runs `test` on a fresh single-threaded executor with a `TestHost` installed, then drains spawned tasks.
/// The virtual clock starts at 1_700_000_000_000 ms. Whenever the test and every spawned task are stalled, the
/// earliest pending timer fires; with no timer pending, the thread parks until something outside wakes a task.
pub fn run<F, Fut, T>(test: F) -> T
where
	F: FnOnce(Rc<TestHost>) -> Fut,
	Fut: Future<Output = T>,
{
	let mut pool = LocalPool::new();
	let host = Rc::new(TestHost {
		handler: RefCell::new(None),
		env: RefCell::new(HashMap::new()),
		clock_ms: RefCell::new(1_700_000_000_000.0),
		requests: RefCell::new(Vec::new()),
		timers: RefCell::new(Vec::new()),
		spawner: pool.spawner(),
	});
	let previous = set_host(host.clone());
	let main = Arc::new(MainWake {
		woken: AtomicBool::new(true),
		thread: std::thread::current(),
	});
	let waker = futures::task::waker(main.clone());
	let mut context = Context::from_waker(&waker);
	let test = test(host.clone());
	futures::pin_mut!(test);
	let output = loop {
		if main.woken.swap(false, Ordering::SeqCst)
			&& let Poll::Ready(output) = test.as_mut().poll(&mut context)
		{
			break output;
		}
		pool.run_until_stalled();
		if main.woken.load(Ordering::SeqCst) || host.fire_next_timer() {
			continue;
		}
		std::thread::park();
	};
	pool.run_until_stalled();
	if let Some(previous) = previous {
		set_host(previous);
	}
	output
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::abort::AbortSignal;

	#[test]
	fn concurrent_timers_fire_in_deadline_order_without_adding_up() {
		let (order, elapsed) = run(|_| async {
			let start = crate::now_ms();
			let order = Rc::new(RefCell::new(Vec::new()));
			let late = order.clone();
			crate::spawn_local(async move {
				crate::sleep(30_000.0, None).await.unwrap();
				late.borrow_mut().push("late");
			});
			let timeout = AbortSignal::timeout(5_000.0);
			crate::sleep(1_000.0, None).await.unwrap();
			order.borrow_mut().push("first");
			assert!(!timeout.aborted());
			crate::sleep(4_000.0, None).await.unwrap();
			assert_eq!(timeout.reason().unwrap().name, "TimeoutError");
			order.borrow_mut().push("second");
			let elapsed = crate::now_ms() - start;
			(order, elapsed)
		});
		assert_eq!(*order.borrow(), vec!["first", "second"]);
		assert_eq!(elapsed, 5_000.0);
	}

	#[test]
	fn a_dropped_sleep_does_not_move_the_clock() {
		let elapsed = run(|_| async {
			let start = crate::now_ms();
			let controller = crate::AbortController::new();
			let signal = controller.signal();
			let sleeping = crate::sleep(60_000.0, Some(&signal));
			controller.abort();
			assert!(sleeping.await.is_err());
			crate::sleep(10.0, None).await.unwrap();
			crate::now_ms() - start
		});
		assert_eq!(elapsed, 10.0);
	}
}
