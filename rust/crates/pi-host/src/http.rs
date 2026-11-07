//! Request/response types for `fetch`, and the `FetchFunction` override that `ProviderRequestOptions.fetch` maps to.

use std::rc::Rc;

use bytes::Bytes;
use futures::StreamExt;
use indexmap::IndexMap;

use crate::abort::AbortSignal;
use crate::error::{PiError, PiResult};
use crate::{LocalBoxFuture, LocalBoxStream};

/// A `fetch` request. Header names keep the caller's casing and order; lookups are case-insensitive.
#[derive(Clone, Debug, Default)]
pub struct HttpRequest {
	pub method: String,
	pub url: String,
	pub headers: IndexMap<String, String>,
	pub body: Option<Bytes>,
	pub signal: Option<AbortSignal>,
}

impl HttpRequest {
	/// A request with `method` and `url` and no headers or body.
	pub fn new(method: impl Into<String>, url: impl Into<String>) -> Self {
		Self {
			method: method.into(),
			url: url.into(),
			..Self::default()
		}
	}

	/// Case-insensitive header lookup, like `Headers.get`.
	pub fn header(&self, name: &str) -> Option<&str> {
		self.headers
			.iter()
			.find(|(key, _)| key.eq_ignore_ascii_case(name))
			.map(|(_, value)| value.as_str())
	}

	/// Sets a header, replacing an existing one with the same name in any casing, like `Headers.set`.
	pub fn set_header(&mut self, name: impl Into<String>, value: impl Into<String>) {
		let name = name.into();
		self.headers.retain(|key, _| !key.eq_ignore_ascii_case(&name));
		self.headers.insert(name, value.into());
	}
}

/// A streamed response body.
pub type Body = LocalBoxStream<'static, PiResult<Bytes>>;

/// A `fetch` response. `headers` keys are lowercased, matching `Headers` iteration in JavaScript.
pub struct HttpResponse {
	pub status: u16,
	pub status_text: String,
	pub url: String,
	pub headers: IndexMap<String, String>,
	pub body: Body,
}

impl HttpResponse {
	/// `response.ok`.
	pub fn ok(&self) -> bool {
		(200..300).contains(&self.status)
	}

	/// `response.headers.get(name)`.
	pub fn header(&self, name: &str) -> Option<&str> {
		self.headers.get(&name.to_ascii_lowercase()).map(String::as_str)
	}

	/// `await response.arrayBuffer()`.
	pub async fn bytes(self) -> PiResult<Vec<u8>> {
		let mut out = Vec::new();
		let mut body = self.body;
		while let Some(chunk) = body.next().await {
			out.extend_from_slice(&chunk?);
		}
		Ok(out)
	}

	/// `await response.text()`: lossy UTF-8 decoding with a leading BOM removed, like `TextDecoder` without `fatal`.
	pub async fn text(self) -> PiResult<String> {
		let bytes = self.bytes().await?;
		let bytes = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]).unwrap_or(&bytes);
		Ok(String::from_utf8_lossy(bytes).into_owned())
	}

	/// `await response.json()`.
	pub async fn json(self) -> PiResult<serde_json::Value> {
		let text = self.text().await?;
		serde_json::from_str(&text).map_err(|error| PiError::named("SyntaxError", error.to_string()))
	}

	/// A response with an in-memory body, for tests and synthesized responses.
	pub fn from_bytes(status: u16, headers: IndexMap<String, String>, body: impl Into<Bytes>) -> Self {
		let body = body.into();
		Self {
			status,
			status_text: String::new(),
			url: String::new(),
			headers: headers
				.into_iter()
				.map(|(key, value)| (key.to_ascii_lowercase(), value))
				.collect(),
			body: futures::stream::once(async move { Ok(body) }).boxed_local(),
		}
	}
}

/// `typeof globalThis.fetch` as an injectable override (`ProviderRequestOptions.fetch`).
pub type FetchFunction = Rc<dyn Fn(HttpRequest) -> LocalBoxFuture<'static, PiResult<HttpResponse>>>;

thread_local! {
	static GLOBAL_FETCH: FetchFunction = Rc::new(|request| Box::pin(crate::host::current().fetch(request)));
}

/// `globalThis.fetch` as a `FetchFunction` value: the current host's fetch. Every call on a thread returns the same
/// `Rc`, so `options.fetch !== globalThis.fetch` is `!is_global_fetch(fetch)`.
pub fn global_fetch() -> FetchFunction {
	GLOBAL_FETCH.with(Rc::clone)
}

/// `fetch === globalThis.fetch`.
pub fn is_global_fetch(fetch: &FetchFunction) -> bool {
	GLOBAL_FETCH.with(|global| Rc::ptr_eq(global, fetch))
}

/// `fetch(request)` through `override_fetch` when given, else the current host.
pub async fn fetch(request: HttpRequest, override_fetch: Option<&FetchFunction>) -> PiResult<HttpResponse> {
	if let Some(signal) = &request.signal {
		signal.throw_if_aborted()?;
	}
	match override_fetch {
		Some(fetch) => fetch(request).await,
		None => crate::host::current().fetch(request).await,
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn global_fetch_is_one_value_per_thread() {
		assert!(is_global_fetch(&global_fetch()));
		let other: FetchFunction = Rc::new(|request| Box::pin(crate::host::current().fetch(request)));
		assert!(!is_global_fetch(&other));
	}
}
