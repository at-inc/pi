//! Test support shared by the pi-mcp suites: a virtual-time host and an in-process HTTP server.
//!
//! `VirtualHost` is the counterpart of vitest's environment: timers only fire once every task has stalled,
//! in deadline order, and the virtual clock jumps to each deadline, so `setTimeout`-based tests (with real or
//! fake timers) run instantly and deterministically. `FakeNetwork` replaces the `node:http` loopback servers of
//! `test/helpers.ts`: `fetch` to a registered origin runs the handler in-process with Node-like request and
//! response objects, and response bodies stream until `end` or until the request's signal aborts.

#![allow(dead_code)]

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::rc::{Rc, Weak};
use std::task::{Context, Poll, Waker};

use bytes::Bytes;
use futures::channel::{mpsc, oneshot};
use futures::executor::LocalPool;
use futures::task::LocalSpawnExt;
use futures::{FutureExt, StreamExt};
use indexmap::IndexMap;
use pi_host::{Host, HttpRequest, HttpResponse, LocalBoxFuture, PiError, PiResult, set_host};
use serde_json::Value;
use url::Url;

/// A JSON value with JavaScript number semantics (`1.0` and `1` are the same), for comparisons.
pub fn norm(value: &Value) -> Value {
	serde_json::from_str(&pi_host::js::json_stringify(value)).expect("stringified JSON parses")
}

/// `JSON.parse` for test literals, normalized like `norm`.
pub fn json(text: &str) -> Value {
	norm(&serde_json::from_str(text).expect("test JSON is valid"))
}

/// Starts `future` now, like calling an async function without awaiting it, and returns its result later.
pub fn spawn<T: 'static>(future: impl Future<Output = T> + 'static) -> LocalBoxFuture<'static, T> {
	let (future, handle) = future.remote_handle();
	pi_host::spawn_local(future);
	Box::pin(handle)
}

/// `new Promise((resolve) => setTimeout(resolve, ms))`.
pub async fn sleep(ms: f64) {
	pi_host::sleep(ms, None).await.expect("no signal");
}

/// A promise and its `resolve` function.
pub struct Gate {
	sender: RefCell<Option<oneshot::Sender<()>>>,
	receiver: RefCell<Option<futures::future::Shared<oneshot::Receiver<()>>>>,
}

impl Gate {
	pub fn new() -> Rc<Gate> {
		let (sender, receiver) = oneshot::channel();
		Rc::new(Gate {
			sender: RefCell::new(Some(sender)),
			receiver: RefCell::new(Some(receiver.shared())),
		})
	}

	pub fn release(&self) {
		if let Some(sender) = self.sender.borrow_mut().take() {
			let _ = sender.send(());
		}
	}

	pub fn wait(&self) -> LocalBoxFuture<'static, ()> {
		let receiver = self.receiver.borrow().clone().expect("gate receiver");
		Box::pin(async move {
			let _ = receiver.await;
		})
	}
}

struct TimerState {
	fired: Cell<bool>,
	waker: RefCell<Option<Waker>>,
}

struct Timer {
	deadline: f64,
	sequence: u64,
	state: Weak<TimerState>,
}

struct Sleep {
	state: Rc<TimerState>,
}

impl Future for Sleep {
	type Output = ();

	fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<()> {
		if self.state.fired.get() {
			return Poll::Ready(());
		}
		*self.state.waker.borrow_mut() = Some(cx.waker().clone());
		Poll::Pending
	}
}

/// The virtual-time test host.
pub struct VirtualHost {
	clock: Cell<f64>,
	timers: RefCell<Vec<Timer>>,
	sequence: Cell<u64>,
	spawner: futures::executor::LocalSpawner,
	pub network: Rc<FakeNetwork>,
	env: RefCell<HashMap<String, String>>,
}

impl VirtualHost {
	/// `Date.now()` of the virtual clock.
	pub fn now(&self) -> f64 {
		self.clock.get()
	}

	pub fn set_env(&self, key: &str, value: &str) {
		self.env.borrow_mut().insert(key.into(), value.into());
	}

	/// Starts an in-process HTTP server and returns its origin.
	pub fn listen(
		&self,
		handler: impl Fn(ServerRequest, ServerResponse) -> LocalBoxFuture<'static, PiResult<()>> + 'static,
	) -> String {
		self.network.listen(handler)
	}

	fn fire_next_timer(&self) -> bool {
		let mut timers = self.timers.borrow_mut();
		timers.retain(|timer| timer.state.upgrade().is_some_and(|state| !state.fired.get()));
		let Some(index) = timers
			.iter()
			.enumerate()
			.min_by(|(_, a), (_, b)| {
				a.deadline
					.partial_cmp(&b.deadline)
					.unwrap_or(std::cmp::Ordering::Equal)
					.then(a.sequence.cmp(&b.sequence))
			})
			.map(|(index, _)| index)
		else {
			return false;
		};
		let timer = timers.remove(index);
		drop(timers);
		if timer.deadline > self.clock.get() {
			self.clock.set(timer.deadline);
		}
		if let Some(state) = timer.state.upgrade() {
			state.fired.set(true);
			if let Some(waker) = state.waker.borrow_mut().take() {
				waker.wake();
			}
		}
		true
	}
}

impl Host for VirtualHost {
	fn fetch(&self, request: HttpRequest) -> LocalBoxFuture<'static, PiResult<HttpResponse>> {
		self.network.fetch(request)
	}

	fn now_ms(&self) -> f64 {
		self.clock.get()
	}

	fn monotonic_ms(&self) -> f64 {
		self.clock.get()
	}

	fn env(&self, key: &str) -> Option<String> {
		self.env.borrow().get(key).cloned()
	}

	fn sleep(&self, ms: f64) -> LocalBoxFuture<'static, ()> {
		let state = Rc::new(TimerState {
			fired: Cell::new(false),
			waker: RefCell::new(None),
		});
		let sequence = self.sequence.get();
		self.sequence.set(sequence + 1);
		let delay = if ms.is_finite() { ms.max(0.0) } else { 0.0 };
		self.timers.borrow_mut().push(Timer {
			deadline: self.clock.get() + delay,
			sequence,
			state: Rc::downgrade(&state),
		});
		Box::pin(Sleep { state })
	}

	fn spawn_local(&self, future: LocalBoxFuture<'static, ()>) {
		self.spawner.spawn_local(future).expect("VirtualHost executor is gone");
	}
}

/// Runs `test` with a `VirtualHost` installed. Panics when every task is stuck and no timer is pending.
/// The virtual clock starts at 1_700_000_000_000 ms.
pub fn run<F, Fut, T>(test: F) -> T
where
	F: FnOnce(Rc<VirtualHost>) -> Fut,
	Fut: Future<Output = T> + 'static,
	T: 'static,
{
	let mut pool = LocalPool::new();
	let host = Rc::new(VirtualHost {
		clock: Cell::new(1_700_000_000_000.0),
		timers: RefCell::new(Vec::new()),
		sequence: Cell::new(0),
		spawner: pool.spawner(),
		network: Rc::new(FakeNetwork::default()),
		env: RefCell::new(HashMap::new()),
	});
	let previous = set_host(host.clone());
	let output: Rc<RefCell<Option<T>>> = Rc::new(RefCell::new(None));
	let slot = output.clone();
	let future = test(host.clone());
	pool.spawner()
		.spawn_local(async move {
			let value = future.await;
			*slot.borrow_mut() = Some(value);
		})
		.expect("spawn test");
	loop {
		pool.run_until_stalled();
		if output.borrow().is_some() {
			break;
		}
		assert!(
			host.fire_next_timer(),
			"test stalled: every task is waiting and no timer is pending"
		);
	}
	if let Some(previous) = previous {
		set_host(previous);
	}
	output.borrow_mut().take().expect("test finished")
}

/// An incoming request as a Node `IncomingMessage` shows it, with the body already read (`readBody`).
#[derive(Clone, Debug)]
pub struct ServerRequest {
	pub method: String,
	pub url: Url,
	/// Lowercased names, like `request.headers`.
	pub headers: IndexMap<String, String>,
	pub body: String,
}

impl ServerRequest {
	/// `request.headers[name]`.
	pub fn header(&self, name: &str) -> Option<&str> {
		self.headers.get(&name.to_ascii_lowercase()).map(String::as_str)
	}

	/// `JSON.parse(await readBody(request))`.
	pub fn json(&self) -> Value {
		norm(&serde_json::from_str(&self.body).expect("request body is JSON"))
	}

	/// `request.url`: the path and query.
	pub fn path(&self) -> &str {
		self.url.path()
	}
}

struct ResponseState {
	status_code: u16,
	headers: IndexMap<String, String>,
	head: Option<oneshot::Sender<(u16, IndexMap<String, String>)>>,
	body: Option<mpsc::UnboundedSender<PiResult<Bytes>>>,
	ended: bool,
}

/// A Node `ServerResponse`.
#[derive(Clone)]
pub struct ServerResponse {
	state: Rc<RefCell<ResponseState>>,
}

impl ServerResponse {
	/// `response.statusCode = status`.
	pub fn set_status(&self, status: u16) {
		self.state.borrow_mut().status_code = status;
	}

	/// `response.setHeader(name, value)`.
	pub fn set_header(&self, name: &str, value: &str) {
		self.state
			.borrow_mut()
			.headers
			.insert(name.to_ascii_lowercase(), value.to_string());
	}

	/// `response.writeHead(status, headers)`.
	pub fn write_head(&self, status: u16, headers: &[(&str, &str)]) -> &Self {
		{
			let mut state = self.state.borrow_mut();
			state.status_code = status;
			for (name, value) in headers {
				state.headers.insert(name.to_ascii_lowercase(), (*value).to_string());
			}
		}
		self.flush_head();
		self
	}

	/// `response.write(chunk)`.
	pub fn write(&self, chunk: &str) {
		self.flush_head();
		if let Some(body) = &self.state.borrow().body {
			let _ = body.unbounded_send(Ok(Bytes::from(chunk.to_string())));
		}
	}

	/// `response.end(chunk)`.
	pub fn end(&self, chunk: &str) {
		if self.state.borrow().ended {
			return;
		}
		if !chunk.is_empty() {
			self.write(chunk);
		}
		self.flush_head();
		let mut state = self.state.borrow_mut();
		state.ended = true;
		state.body = None;
	}

	/// Whether `end` or `writeHead` already sent the status line.
	pub fn headers_sent(&self) -> bool {
		self.state.borrow().head.is_none()
	}

	fn flush_head(&self) {
		let mut state = self.state.borrow_mut();
		if let Some(head) = state.head.take() {
			let _ = head.send((state.status_code, state.headers.clone()));
		}
	}
}

type ServerHandler = Rc<dyn Fn(ServerRequest, ServerResponse) -> LocalBoxFuture<'static, PiResult<()>>>;

/// In-process HTTP servers, addressed by origin.
#[derive(Default)]
pub struct FakeNetwork {
	servers: RefCell<IndexMap<String, ServerHandler>>,
	/// Responses stay open after their handler returns, like a Node socket that was not ended.
	responses: RefCell<Vec<ServerResponse>>,
}

impl FakeNetwork {
	/// `listen(handler)` from `test/helpers.ts`: registers a server and returns its origin.
	pub fn listen(
		&self,
		handler: impl Fn(ServerRequest, ServerResponse) -> LocalBoxFuture<'static, PiResult<()>> + 'static,
	) -> String {
		let mut servers = self.servers.borrow_mut();
		// Below the ephemeral range, so a real socket of the same test (the OAuth callback server) never shares it.
		let origin = format!("http://127.0.0.1:{}", 100 + servers.len());
		servers.insert(origin.clone(), Rc::new(handler));
		origin
	}

	/// Whether `url` belongs to one of the servers.
	pub fn serves(&self, url: &str) -> bool {
		Url::parse(url).is_ok_and(|url| self.servers.borrow().contains_key(&url.origin().ascii_serialization()))
	}

	pub fn fetch(&self, request: HttpRequest) -> LocalBoxFuture<'static, PiResult<HttpResponse>> {
		let url = match Url::parse(&request.url) {
			Ok(url) => url,
			Err(error) => return Box::pin(async move { Err(PiError::type_error(format!("Invalid URL: {error}"))) }),
		};
		let handler = self.servers.borrow().get(&url.origin().ascii_serialization()).cloned();
		let Some(handler) = handler else {
			return Box::pin(async { Err(PiError::type_error("fetch failed")) });
		};
		let (head_sender, head_receiver) = oneshot::channel();
		let (body_sender, body_receiver) = mpsc::unbounded();
		let response = ServerResponse {
			state: Rc::new(RefCell::new(ResponseState {
				status_code: 200,
				headers: IndexMap::new(),
				head: Some(head_sender),
				body: Some(body_sender),
				ended: false,
			})),
		};
		let server_request = ServerRequest {
			method: request.method.clone(),
			url: url.clone(),
			headers: request
				.headers
				.iter()
				.map(|(name, value)| (name.to_ascii_lowercase(), value.clone()))
				.collect(),
			body: request
				.body
				.as_ref()
				.map(|body| String::from_utf8_lossy(body).into_owned())
				.unwrap_or_default(),
		};
		self.responses.borrow_mut().push(response.clone());
		let handler_response = response.clone();
		pi_host::spawn_local(async move {
			if let Err(error) = handler(server_request, handler_response.clone()).await {
				handler_response.set_status(500);
				handler_response.end(&error.to_string());
			}
		});
		let signal = request.signal.clone();
		let url = request.url.clone();
		Box::pin(async move {
			let head = match &signal {
				Some(signal) => {
					let aborted = signal.aborted_future();
					futures::pin_mut!(aborted);
					match futures::future::select(head_receiver, aborted).await {
						futures::future::Either::Left((head, _)) => head,
						futures::future::Either::Right((reason, _)) => return Err(reason),
					}
				}
				None => head_receiver.await,
			};
			let (status, headers) = head.map_err(|_| PiError::type_error("fetch failed: socket hang up"))?;
			let body = match signal {
				Some(signal) => pi_host::sse::async_stream(move |yield_| async move {
					let mut receiver = body_receiver;
					loop {
						let next = receiver.next();
						let aborted = signal.aborted_future();
						futures::pin_mut!(next, aborted);
						match futures::future::select(next, aborted).await {
							futures::future::Either::Left((Some(item), _)) => yield_(item).await,
							futures::future::Either::Left((None, _)) => break,
							futures::future::Either::Right((reason, _)) => {
								yield_(Err(reason)).await;
								break;
							}
						}
					}
				}),
				None => body_receiver.boxed_local(),
			};
			Ok(HttpResponse {
				status,
				status_text: String::new(),
				url,
				headers,
				body,
			})
		})
	}
}

/// A native host whose `fetch` reaches `FakeNetwork` servers in-process and everything else over the real
/// network, for tests that mix in-process servers with real sockets (the OAuth callback server).
#[cfg(feature = "native")]
pub struct NetworkHost {
	native: pi_host::native::NativeHost,
	pub network: Rc<FakeNetwork>,
}

#[cfg(feature = "native")]
impl NetworkHost {
	/// Starts an in-process HTTP server and returns its origin.
	pub fn listen(
		&self,
		handler: impl Fn(ServerRequest, ServerResponse) -> LocalBoxFuture<'static, PiResult<()>> + 'static,
	) -> String {
		self.network.listen(handler)
	}
}

#[cfg(feature = "native")]
impl Host for NetworkHost {
	fn fetch(&self, request: HttpRequest) -> LocalBoxFuture<'static, PiResult<HttpResponse>> {
		if self.network.serves(&request.url) {
			return self.network.fetch(request);
		}
		self.native.fetch(request)
	}

	fn now_ms(&self) -> f64 {
		self.native.now_ms()
	}

	fn monotonic_ms(&self) -> f64 {
		self.native.monotonic_ms()
	}

	fn env(&self, key: &str) -> Option<String> {
		self.native.env(key)
	}

	fn sleep(&self, ms: f64) -> LocalBoxFuture<'static, ()> {
		self.native.sleep(ms)
	}

	fn spawn_local(&self, future: LocalBoxFuture<'static, ()>) {
		self.native.spawn_local(future);
	}
}

/// Runs `test` on a current-thread tokio runtime inside a `LocalSet`, with a `NetworkHost` installed.
#[cfg(feature = "native")]
pub fn run_native<F, Fut, T>(test: F) -> T
where
	F: FnOnce(Rc<NetworkHost>) -> Fut,
	Fut: Future<Output = T>,
{
	let runtime = tokio::runtime::Builder::new_current_thread()
		.enable_all()
		.build()
		.expect("tokio runtime");
	let host = Rc::new(NetworkHost {
		native: pi_host::native::NativeHost::new(),
		network: Rc::new(FakeNetwork::default()),
	});
	let previous = set_host(host.clone());
	let output = tokio::task::LocalSet::new().block_on(&runtime, test(host));
	if let Some(previous) = previous {
		set_host(previous);
	}
	output
}
