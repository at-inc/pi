//! Scripted host for tests: a programmable `fetch`, scripted WebSockets, a virtual clock, an in-memory environment,
//! and a single-threaded executor. Ported vitest suites that stub `globalThis.fetch` or use fake timers use this.

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::pin::{Pin, pin};
use std::rc::{Rc, Weak};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::task::{Context, Poll, Wake, Waker};
use std::thread::Thread;

use futures::executor::LocalPool;
use futures::task::LocalSpawnExt;
use indexmap::IndexMap;

use crate::LocalBoxFuture;
use crate::error::{PiError, PiResult};
use crate::host::{Host, set_host};
use crate::http::{HttpRequest, HttpResponse};
use crate::websocket::{WebSocket, WebSocketEvent, WebSocketListener, WebSocketListenerId, WebSocketListeners};

type Handler = Rc<dyn Fn(HttpRequest) -> LocalBoxFuture<'static, PiResult<HttpResponse>>>;

/// The test host. Timers are virtual, like fake timers in vitest: a sleep is due at the virtual clock plus its
/// delay, and `run` fires the earliest due timer whenever every task is waiting, advancing the clock to its due
/// time. So retry and backoff logic runs instantly while `now_ms` still moves forward, and concurrent sleeps
/// resolve in due-time order (then in the order they were started), like `setTimeout`.
pub struct TestHost {
	handler: RefCell<Option<Handler>>,
	websocket_handler: RefCell<Option<WebSocketHandler>>,
	env: RefCell<HashMap<String, String>>,
	clock_ms: RefCell<f64>,
	requests: RefCell<Vec<RecordedRequest>>,
	files: RefCell<HashSet<String>>,
	home_dir: RefCell<Option<String>>,
	spawner: futures::executor::LocalSpawner,
	timers: RefCell<Vec<Timer>>,
	next_timer: Cell<u64>,
}

struct Timer {
	due_ms: f64,
	order: u64,
	state: Rc<RefCell<TimerState>>,
}

#[derive(Default)]
struct TimerState {
	fired: bool,
	cancelled: bool,
	waker: Option<Waker>,
}

/// A pending virtual timer. Dropping it cancels the timer, so a lost race never moves the clock.
struct Sleep {
	state: Rc<RefCell<TimerState>>,
}

impl Future for Sleep {
	type Output = ();

	fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<()> {
		let mut state = self.state.borrow_mut();
		if state.fired {
			return Poll::Ready(());
		}
		state.waker = Some(cx.waker().clone());
		Poll::Pending
	}
}

impl Drop for Sleep {
	fn drop(&mut self) {
		self.state.borrow_mut().cancelled = true;
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

	/// Installs `globalThis.WebSocket`: `handler` plays the constructor, receiving the URL and the `headers` option.
	/// Without a handler the host has no WebSocket.
	pub fn on_websocket(
		&self,
		handler: impl Fn(&str, &IndexMap<String, String>) -> PiResult<Rc<FakeWebSocket>> + 'static,
	) {
		*self.websocket_handler.borrow_mut() = Some(Rc::new(handler));
	}

	/// Sets `process.env[key]`.
	pub fn set_env(&self, key: &str, value: &str) {
		self.env.borrow_mut().insert(key.into(), value.into());
	}

	/// Every request `fetch` received so far.
	pub fn requests(&self) -> Vec<RecordedRequest> {
		self.requests.borrow().clone()
	}

	/// Moves the virtual clock forward. Timers that become due fire the next time every task is waiting.
	pub fn advance(&self, ms: f64) {
		*self.clock_ms.borrow_mut() += ms;
	}

	/// Makes `file_exists(path)` true.
	pub fn add_file(&self, path: &str) {
		self.files.borrow_mut().insert(path.into());
	}

	/// Sets `os.homedir()`. Without it the host has no file system, like a browser.
	pub fn set_home_dir(&self, dir: &str) {
		*self.home_dir.borrow_mut() = Some(dir.into());
	}

	/// Fires the earliest pending timer, moving the clock to its due time if that is later. Returns `false` when no
	/// timer is pending.
	fn fire_next_timer(&self) -> bool {
		let next = {
			let mut timers = self.timers.borrow_mut();
			timers.retain(|timer| !timer.state.borrow().cancelled);
			let index = timers
				.iter()
				.enumerate()
				.min_by(|(_, a), (_, b)| a.due_ms.total_cmp(&b.due_ms).then(a.order.cmp(&b.order)))
				.map(|(index, _)| index);
			index.map(|index| timers.remove(index))
		};
		let Some(timer) = next else {
			return false;
		};
		{
			let mut clock = self.clock_ms.borrow_mut();
			*clock = clock.max(timer.due_ms);
		}
		let waker = {
			let mut state = timer.state.borrow_mut();
			state.fired = true;
			state.waker.take()
		};
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
		let state = Rc::new(RefCell::new(TimerState::default()));
		let order = self.next_timer.get();
		self.next_timer.set(order + 1);
		self.timers.borrow_mut().push(Timer {
			due_ms: *self.clock_ms.borrow() + ms.max(0.0),
			order,
			state: state.clone(),
		});
		Box::pin(Sleep { state })
	}

	fn spawn_local(&self, future: LocalBoxFuture<'static, ()>) {
		self.spawner.spawn_local(future).expect("TestHost executor is gone");
	}

	fn file_exists(&self, path: &str) -> bool {
		self.files.borrow().contains(path)
	}

	fn home_dir(&self) -> Option<String> {
		self.home_dir.borrow().clone()
	}

	fn websocket(&self, url: &str, headers: &IndexMap<String, String>) -> Option<PiResult<Rc<dyn WebSocket>>> {
		let handler = self.websocket_handler.borrow().clone()?;
		Some(handler(url, headers).map(|socket| socket as Rc<dyn WebSocket>))
	}
}

type WebSocketHandler = Rc<dyn Fn(&str, &IndexMap<String, String>) -> PiResult<Rc<FakeWebSocket>>>;
type SendHandler = Rc<dyn Fn(&Rc<FakeWebSocket>, &str) -> PiResult<()>>;
type CloseHandler = Rc<dyn Fn(&Rc<FakeWebSocket>, u16, &str)>;

/// A scripted WebSocket for tests, the counterpart of the `MockWebSocket` classes vitest suites stub
/// `globalThis.WebSocket` with. Tests dispatch its events with `emit` (synchronously) or `emit_later` (from a task,
/// like `queueMicrotask`) and react to `send` and `close` through `on_send` and `on_close`. Its `readyState` is
/// absent until `set_ready_state` sets one.
pub struct FakeWebSocket {
	pub url: String,
	pub headers: IndexMap<String, String>,
	this: Weak<FakeWebSocket>,
	ready_state: Cell<Option<u16>>,
	listeners: WebSocketListeners,
	sent: RefCell<Vec<String>>,
	closes: RefCell<Vec<(u16, String)>>,
	on_send: RefCell<Option<SendHandler>>,
	on_close: RefCell<Option<CloseHandler>>,
}

impl FakeWebSocket {
	/// A socket for `url` opened with `headers`, without listeners, handlers or `readyState`.
	pub fn new(url: &str, headers: &IndexMap<String, String>) -> Rc<Self> {
		Rc::new_cyclic(|this| Self {
			url: url.to_string(),
			headers: headers.clone(),
			this: this.clone(),
			ready_state: Cell::new(None),
			listeners: WebSocketListeners::default(),
			sent: RefCell::new(Vec::new()),
			closes: RefCell::new(Vec::new()),
			on_send: RefCell::new(None),
			on_close: RefCell::new(None),
		})
	}

	/// Sets `readyState`; `None` removes it.
	pub fn set_ready_state(&self, state: Option<u16>) {
		self.ready_state.set(state);
	}

	/// Handles `send`; an `Err` is a throw from `send`.
	pub fn on_send(&self, handler: impl Fn(&Rc<FakeWebSocket>, &str) -> PiResult<()> + 'static) {
		*self.on_send.borrow_mut() = Some(Rc::new(handler));
	}

	/// Handles `close`.
	pub fn on_close(&self, handler: impl Fn(&Rc<FakeWebSocket>, u16, &str) + 'static) {
		*self.on_close.borrow_mut() = Some(Rc::new(handler));
	}

	/// Dispatches `event` to the listeners registered now.
	pub fn emit(&self, event: WebSocketEvent) {
		self.listeners.dispatch(&event);
	}

	/// Dispatches `events` in order from a spawned task, after the current task yields.
	pub fn emit_later(self: &Rc<Self>, events: Vec<WebSocketEvent>) {
		let socket = self.clone();
		crate::host::spawn_local(async move {
			for event in events {
				socket.emit(event);
			}
		});
	}

	/// Every text frame sent so far.
	pub fn sent(&self) -> Vec<String> {
		self.sent.borrow().clone()
	}

	/// Every `close(code, reason)` call so far.
	pub fn closes(&self) -> Vec<(u16, String)> {
		self.closes.borrow().clone()
	}

	fn rc(&self) -> Rc<FakeWebSocket> {
		self.this
			.upgrade()
			.expect("FakeWebSocket is alive while one of its methods runs")
	}
}

impl WebSocket for FakeWebSocket {
	fn ready_state(&self) -> Option<u16> {
		self.ready_state.get()
	}

	fn send(&self, data: &str) -> PiResult<()> {
		self.sent.borrow_mut().push(data.to_string());
		let handler = self.on_send.borrow().clone();
		match handler {
			Some(handler) => handler(&self.rc(), data),
			None => Ok(()),
		}
	}

	fn close(&self, code: u16, reason: &str) -> PiResult<()> {
		self.closes.borrow_mut().push((code, reason.to_string()));
		let handler = self.on_close.borrow().clone();
		if let Some(handler) = handler {
			handler(&self.rc(), code, reason);
		}
		Ok(())
	}

	fn add_event_listener(&self, listener: WebSocketListener) -> WebSocketListenerId {
		self.listeners.add(listener)
	}

	fn remove_event_listener(&self, id: WebSocketListenerId) {
		self.listeners.remove(id);
	}
}

/// Wakes the test future: sets its flag and unparks the thread that runs it.
struct MainWaker {
	woken: AtomicBool,
	thread: Thread,
}

impl Wake for MainWaker {
	fn wake(self: Arc<Self>) {
		self.wake_by_ref();
	}

	fn wake_by_ref(self: &Arc<Self>) {
		self.woken.store(true, Ordering::SeqCst);
		self.thread.unpark();
	}
}

/// Runs `test` on a fresh single-threaded executor with a `TestHost` installed, then drains spawned tasks and
/// pending timers. Whenever the test future and every spawned task are waiting, the earliest pending timer fires.
/// The virtual clock starts at 1_700_000_000_000 ms.
pub fn run<F, Fut, T>(test: F) -> T
where
	F: FnOnce(Rc<TestHost>) -> Fut,
	Fut: Future<Output = T>,
{
	let mut pool = LocalPool::new();
	let host = Rc::new(TestHost {
		handler: RefCell::new(None),
		websocket_handler: RefCell::new(None),
		env: RefCell::new(HashMap::new()),
		clock_ms: RefCell::new(1_700_000_000_000.0),
		requests: RefCell::new(Vec::new()),
		files: RefCell::new(HashSet::new()),
		home_dir: RefCell::new(None),
		spawner: pool.spawner(),
		timers: RefCell::new(Vec::new()),
		next_timer: Cell::new(0),
	});
	let previous = set_host(host.clone());
	let main_waker = Arc::new(MainWaker {
		woken: AtomicBool::new(true),
		thread: std::thread::current(),
	});
	let waker = Waker::from(main_waker.clone());
	let mut cx = Context::from_waker(&waker);
	let mut main = pin!(test(host.clone()));
	let output = loop {
		if main_waker.woken.swap(false, Ordering::SeqCst)
			&& let Poll::Ready(output) = main.as_mut().poll(&mut cx)
		{
			break output;
		}
		pool.run_until_stalled();
		if main_waker.woken.load(Ordering::SeqCst) || host.fire_next_timer() {
			continue;
		}
		std::thread::park();
	};
	loop {
		pool.run_until_stalled();
		if !host.fire_next_timer() {
			break;
		}
	}
	if let Some(previous) = previous {
		set_host(previous);
	}
	output
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn concurrent_sleeps_resolve_in_due_order_and_move_the_clock() {
		let order = run(|host| async move {
			let order = Rc::new(RefCell::new(Vec::new()));
			let start = host.monotonic_ms();
			let task = |ms: f64, label: &'static str| {
				let order = order.clone();
				let host = host.clone();
				async move {
					crate::sleep(ms, None).await.unwrap();
					order.borrow_mut().push((label, host.monotonic_ms() - start));
				}
			};
			futures::join!(task(30.0, "slow"), task(10.0, "fast"), task(10.0, "fast-second"));
			order.borrow().clone()
		});
		assert_eq!(order, vec![("fast", 10.0), ("fast-second", 10.0), ("slow", 30.0)]);
	}

	#[test]
	fn a_dropped_sleep_does_not_move_the_clock() {
		let elapsed = run(|host| async move {
			let start = host.monotonic_ms();
			let lost = futures::future::select(Box::pin(crate::sleep(1_000.0, None)), futures::future::ready(())).await;
			drop(lost);
			crate::sleep(5.0, None).await.unwrap();
			host.monotonic_ms() - start
		});
		assert_eq!(elapsed, 5.0);
	}
}
