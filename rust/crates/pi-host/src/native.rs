//! Native host: reqwest for `fetch`, tokio for timers and local tasks, `std::env` for the environment.
//! Must run inside a tokio `LocalSet` (see `run`), because ported code spawns `!Send` futures.

use std::future::Future;
use std::rc::Rc;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use futures::{StreamExt, TryStreamExt};

use crate::LocalBoxFuture;
use crate::error::{PiError, PiResult};
use crate::host::{Host, set_host};
use crate::http::{HttpRequest, HttpResponse, RedirectMode};

/// The native host.
pub struct NativeHost {
	client: reqwest::Client,
	manual_client: reqwest::Client,
	started: Instant,
}

impl NativeHost {
	/// A host with a default reqwest client.
	pub fn new() -> Self {
		Self {
			client: reqwest::Client::builder()
				.redirect(reqwest::redirect::Policy::limited(20))
				.build()
				.expect("reqwest client"),
			manual_client: reqwest::Client::builder()
				.redirect(reqwest::redirect::Policy::none())
				.build()
				.expect("reqwest client"),
			started: Instant::now(),
		}
	}
}

impl Default for NativeHost {
	fn default() -> Self {
		Self::new()
	}
}

impl Host for NativeHost {
	fn fetch(&self, request: HttpRequest) -> LocalBoxFuture<'static, PiResult<HttpResponse>> {
		let redirect = request.redirect;
		let client = match redirect {
			RedirectMode::Follow => self.client.clone(),
			RedirectMode::Manual | RedirectMode::Error => self.manual_client.clone(),
		};
		Box::pin(async move {
			let method = reqwest::Method::from_bytes(request.method.as_bytes())
				.map_err(|_| PiError::type_error(format!("Invalid method: {}", request.method)))?;
			let mut builder = client.request(method, &request.url);
			for (name, value) in &request.headers {
				builder = builder.header(name, value);
			}
			if let Some(body) = request.body {
				builder = builder.body(body);
			}
			let send = builder.send();
			let response = match &request.signal {
				Some(signal) => {
					let aborted = signal.aborted_future();
					futures::pin_mut!(send, aborted);
					match futures::future::select(send, aborted).await {
						futures::future::Either::Left((response, _)) => response,
						futures::future::Either::Right((reason, _)) => return Err(reason),
					}
				}
				None => send.await,
			}
			.map_err(|error| {
				PiError::type_error("fetch failed").with_data(serde_json::json!({ "cause": error.to_string() }))
			})?;
			let status = response.status();
			if redirect == RedirectMode::Error && status.is_redirection() && response.headers().contains_key("location")
			{
				return Err(PiError::type_error("fetch failed")
					.with_data(serde_json::json!({ "cause": "unexpected redirect" })));
			}
			let mut headers: indexmap::IndexMap<String, String> = indexmap::IndexMap::new();
			for (name, value) in response.headers() {
				let value = String::from_utf8_lossy(value.as_bytes()).into_owned();
				headers
					.entry(name.as_str().to_string())
					.and_modify(|existing| {
						existing.push_str(", ");
						existing.push_str(&value);
					})
					.or_insert(value);
			}
			let url = response.url().to_string();
			let signal = request.signal.clone();
			let body = response.bytes_stream().map_err(|error| {
				PiError::type_error("terminated").with_data(serde_json::json!({ "cause": error.to_string() }))
			});
			let body = match signal {
				Some(signal) => futures::stream::unfold(Some((body.boxed_local(), signal)), |state| async move {
					let (mut body, signal) = state?;
					let watcher = signal.clone();
					let aborted = watcher.aborted_future();
					futures::pin_mut!(aborted);
					match futures::future::select(body.next(), aborted).await {
						futures::future::Either::Left((Some(chunk), _)) => Some((chunk, Some((body, signal)))),
						futures::future::Either::Left((None, _)) => None,
						futures::future::Either::Right((reason, _)) => Some((Err(reason), None)),
					}
				})
				.boxed_local(),
				None => body.boxed_local(),
			};
			Ok(HttpResponse {
				status: status.as_u16(),
				status_text: status.canonical_reason().unwrap_or("").to_string(),
				url,
				headers,
				body,
			})
		})
	}

	fn now_ms(&self) -> f64 {
		SystemTime::now()
			.duration_since(UNIX_EPOCH)
			.map(|duration| duration.as_millis() as f64)
			.unwrap_or(0.0)
	}

	fn monotonic_ms(&self) -> f64 {
		self.started.elapsed().as_secs_f64() * 1000.0
	}

	fn env(&self, key: &str) -> Option<String> {
		std::env::var(key).ok()
	}

	fn sleep(&self, ms: f64) -> LocalBoxFuture<'static, ()> {
		let ms = if ms.is_finite() && (1.0..=2_147_483_647.0).contains(&ms) {
			ms
		} else {
			1.0
		};
		Box::pin(tokio::time::sleep(std::time::Duration::from_secs_f64(ms / 1000.0)))
	}

	fn spawn_local(&self, future: LocalBoxFuture<'static, ()>) {
		tokio::task::spawn_local(future);
	}

	fn file_exists(&self, path: &str) -> bool {
		std::path::Path::new(path).exists()
	}

	fn home_dir(&self) -> Option<String> {
		std::env::home_dir().map(|dir| dir.to_string_lossy().into_owned())
	}

	fn read_file(&self, path: &str) -> LocalBoxFuture<'static, PiResult<String>> {
		let path = path.to_string();
		Box::pin(async move {
			match tokio::fs::read(&path).await {
				Ok(bytes) => Ok(String::from_utf8_lossy(&bytes).into_owned()),
				Err(error) => Err(match error.kind() {
					std::io::ErrorKind::NotFound => crate::host::file_not_found(&path),
					std::io::ErrorKind::PermissionDenied => PiError::new(format!(
						"EACCES: permission denied, open '{path}'"
					))
					.with_data(serde_json::json!({ "errno": -13, "code": "EACCES", "syscall": "open", "path": path })),
					std::io::ErrorKind::IsADirectory => PiError::new("EISDIR: illegal operation on a directory, read")
						.with_data(serde_json::json!({ "errno": -21, "code": "EISDIR", "syscall": "read" })),
					_ => PiError::new(error.to_string()),
				}),
			}
		})
	}
}

/// Runs `main` on a current-thread tokio runtime inside a `LocalSet`, with a `NativeHost` installed.
pub fn run<F, Fut, T>(main: F) -> T
where
	F: FnOnce() -> Fut,
	Fut: Future<Output = T>,
{
	let runtime = tokio::runtime::Builder::new_current_thread()
		.enable_all()
		.build()
		.expect("tokio runtime");
	set_host(Rc::new(NativeHost::new()));
	tokio::task::LocalSet::new().block_on(&runtime, main())
}
