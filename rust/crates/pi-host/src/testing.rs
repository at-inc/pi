//! Scripted host for tests: a programmable `fetch`, a virtual clock, an in-memory environment, and a
//! single-threaded executor. Ported vitest suites that stub `globalThis.fetch` or use fake timers use this.

use std::cell::RefCell;
use std::collections::HashMap;
use std::future::Future;
use std::rc::Rc;

use futures::executor::LocalPool;
use futures::task::LocalSpawnExt;

use crate::LocalBoxFuture;
use crate::error::{PiError, PiResult};
use crate::host::{Host, set_host};
use crate::http::{HttpRequest, HttpResponse};

type Handler = Rc<dyn Fn(HttpRequest) -> LocalBoxFuture<'static, PiResult<HttpResponse>>>;

/// The test host. Sleeping advances the virtual clock and resolves on the next executor turn, so retry and
/// backoff logic runs instantly while `now_ms` still moves forward.
pub struct TestHost {
	handler: RefCell<Option<Handler>>,
	env: RefCell<HashMap<String, String>>,
	clock_ms: RefCell<f64>,
	requests: RefCell<Vec<RecordedRequest>>,
	spawner: futures::executor::LocalSpawner,
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

	/// Moves the virtual clock forward.
	pub fn advance(&self, ms: f64) {
		*self.clock_ms.borrow_mut() += ms;
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
		*self.clock_ms.borrow()
	}

	fn monotonic_ms(&self) -> f64 {
		*self.clock_ms.borrow()
	}

	fn env(&self, key: &str) -> Option<String> {
		self.env.borrow().get(key).cloned()
	}

	fn sleep(&self, ms: f64) -> LocalBoxFuture<'static, ()> {
		self.advance(ms.max(0.0));
		Box::pin(futures::future::ready(()))
	}

	fn spawn_local(&self, future: LocalBoxFuture<'static, ()>) {
		self.spawner.spawn_local(future).expect("TestHost executor is gone");
	}
}

/// Runs `test` on a fresh single-threaded executor with a `TestHost` installed, then drains spawned tasks.
/// The virtual clock starts at 1_700_000_000_000 ms.
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
		spawner: pool.spawner(),
	});
	let previous = set_host(host.clone());
	let output = pool.run_until(test(host));
	pool.run_until_stalled();
	if let Some(previous) = previous {
		set_host(previous);
	}
	output
}
