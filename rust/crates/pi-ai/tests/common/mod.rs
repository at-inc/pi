//! Test support for the OAuth flow suites: a host whose `fetch` is a test handler.
//!
//! vitest stubs `globalThis.fetch` while the flows' callback servers listen on real sockets. Without the `native`
//! feature the suites run on `pi_host::testing` (no socket: the flows fall back to manual input). With `native` they
//! run on the tokio host with `fetch` replaced by the handler, which can pass requests on through `native_fetch`.

#![allow(dead_code)]

use std::future::Future;
use std::rc::Rc;

use indexmap::IndexMap;
use pi_host::{HttpRequest, HttpResponse, LocalBoxFuture, PiResult};
use serde_json::Value;

/// Serializes tests that bind fixed loopback ports, like vitest's `describe.sequential`.
pub fn sequential() -> std::sync::MutexGuard<'static, ()> {
	static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
	LOCK.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

pub type FetchHandler = Rc<dyn Fn(HttpRequest) -> LocalBoxFuture<'static, PiResult<HttpResponse>>>;

/// `new Response(JSON.stringify(body), { status, headers: { "Content-Type": "application/json" } })`.
pub fn json_response(body: Value, status: u16) -> HttpResponse {
	let mut headers = IndexMap::new();
	headers.insert("Content-Type".to_string(), "application/json".to_string());
	HttpResponse::from_bytes(status, headers, pi_host::js::json_stringify(&body))
}

/// `JSON.parse(init.body)` of a recorded request.
pub fn json_body(request: &HttpRequest) -> Value {
	serde_json::from_slice(request.body.as_deref().expect("request has a body")).expect("request body is JSON")
}

/// The form fields of an `application/x-www-form-urlencoded` request body.
pub fn form_body(request: &HttpRequest) -> IndexMap<String, String> {
	url::form_urlencoded::parse(request.body.as_deref().expect("request has a body"))
		.into_owned()
		.collect()
}

#[cfg(feature = "native")]
mod host {
	use std::future::Future;

	use pi_host::native::NativeHost;
	use pi_host::{Host, HttpRequest, HttpResponse, LocalBoxFuture, PiResult};

	use super::FetchHandler;

	struct FetchHost {
		native: NativeHost,
		handler: FetchHandler,
		env: Vec<(String, String)>,
	}

	impl Host for FetchHost {
		fn fetch(&self, request: HttpRequest) -> LocalBoxFuture<'static, PiResult<HttpResponse>> {
			(self.handler)(request)
		}
		fn now_ms(&self) -> f64 {
			self.native.now_ms()
		}
		fn monotonic_ms(&self) -> f64 {
			self.native.monotonic_ms()
		}
		fn env(&self, key: &str) -> Option<String> {
			match self.env.iter().find(|(name, _)| name == key) {
				Some((_, value)) => Some(value.clone()),
				None => self.native.env(key),
			}
		}
		fn sleep(&self, ms: f64) -> LocalBoxFuture<'static, ()> {
			self.native.sleep(ms)
		}
		fn spawn_local(&self, future: LocalBoxFuture<'static, ()>) {
			self.native.spawn_local(future)
		}
	}

	pub fn run<T, Fut: Future<Output = T>>(
		handler: FetchHandler,
		env: &[(&str, &str)],
		test: impl FnOnce() -> Fut,
	) -> T {
		let env = env
			.iter()
			.map(|(name, value)| (name.to_string(), value.to_string()))
			.collect();
		pi_host::native::run(|| async move {
			let previous = pi_host::set_host(std::rc::Rc::new(FetchHost {
				native: NativeHost::new(),
				handler,
				env,
			}));
			let output = test().await;
			if let Some(previous) = previous {
				pi_host::set_host(previous);
			}
			output
		})
	}

	pub fn native_fetch(request: HttpRequest) -> LocalBoxFuture<'static, PiResult<HttpResponse>> {
		NativeHost::new().fetch(request)
	}
}

#[cfg(not(feature = "native"))]
mod host {
	use std::future::Future;

	use pi_host::{HttpRequest, HttpResponse, LocalBoxFuture, PiError, PiResult};

	use super::FetchHandler;

	pub fn run<T, Fut: Future<Output = T>>(
		handler: FetchHandler,
		env: &[(&str, &str)],
		test: impl FnOnce() -> Fut,
	) -> T {
		pi_host::testing::run(|host| {
			host.on_fetch(move |request| handler(request));
			for (name, value) in env {
				host.set_env(name, value);
			}
			test()
		})
	}

	pub fn native_fetch(_request: HttpRequest) -> LocalBoxFuture<'static, PiResult<HttpResponse>> {
		Box::pin(async {
			Err(PiError::type_error(
				"fetch failed: no network without the native feature",
			))
		})
	}
}

/// Runs `test` with `fetch` served by `handler`.
pub fn run<T, Fut: Future<Output = T>>(handler: FetchHandler, test: impl FnOnce() -> Fut) -> T {
	host::run(handler, &[], test)
}

/// `run` with `vi.stubEnv` for each of `env`.
pub fn run_with_env<T, Fut: Future<Output = T>>(
	handler: FetchHandler,
	env: &[(&str, &str)],
	test: impl FnOnce() -> Fut,
) -> T {
	host::run(handler, env, test)
}

/// `run` on `pi_host::testing` with the `native` feature too, for suites that use vitest's fake timers.
pub fn run_with_fake_timers<T, Fut: Future<Output = T>>(handler: FetchHandler, test: impl FnOnce() -> Fut) -> T {
	pi_host::testing::run(|host| {
		host.on_fetch(move |request| handler(request));
		test()
	})
}

/// The platform `fetch` the handler stubbed out.
pub fn native_fetch(request: HttpRequest) -> LocalBoxFuture<'static, PiResult<HttpResponse>> {
	host::native_fetch(request)
}
