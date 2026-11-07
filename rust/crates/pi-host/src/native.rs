//! Native host: reqwest for `fetch`, tokio for timers and local tasks, `std::env` for the environment, and
//! tokio-tungstenite for `WebSocket` (rustls with the webpki roots, like reqwest; no HTTP proxy support).
//! Must run inside a tokio `LocalSet` (see `run`), because ported code spawns `!Send` futures. `listen_http` is a
//! tokio `TcpListener` (see `native/server.rs`).

mod server;

use std::cell::Cell;
use std::future::Future;
use std::rc::Rc;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use futures::{FutureExt, StreamExt, TryStreamExt};
use indexmap::IndexMap;

use crate::LocalBoxFuture;
use crate::error::{PiError, PiResult};
use crate::host::{Host, set_host};
use crate::http::{HttpRequest, HttpResponse, RedirectMode};
use crate::server::{HttpServer, HttpServerErrorHandler, HttpServerHandler};

use crate::websocket::{
	CLOSED, CLOSING, CONNECTING, OPEN, WebSocket, WebSocketData, WebSocketEvent, WebSocketListener,
	WebSocketListenerId, WebSocketListeners,
};

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

	fn websocket(&self, url: &str, headers: &IndexMap<String, String>) -> Option<PiResult<Rc<dyn WebSocket>>> {
		Some(NativeWebSocket::connect(url, headers))
	}

	fn spawn_process(
		&self,
		options: crate::process::SpawnOptions,
		listeners: crate::process::ProcessListeners,
	) -> LocalBoxFuture<'static, PiResult<Rc<dyn crate::process::ChildProcess>>> {
		let spawned = crate::native_process::spawn(options, listeners);
		Box::pin(async move { spawned })
	}

	fn listen_http(
		&self,
		host: &str,
		port: u16,
		handler: HttpServerHandler,
		on_error: HttpServerErrorHandler,
	) -> LocalBoxFuture<'static, PiResult<Rc<dyn HttpServer>>> {
		Box::pin(server::listen(host.to_string(), port, handler, on_error))
	}
}

/// A native WebSocket client socket over tokio-tungstenite. The connection runs on a local task that dispatches the
/// socket's events; `send` and `close` queue frames for it.
struct NativeWebSocket {
	state: Cell<u16>,
	listeners: WebSocketListeners,
	outgoing: futures::channel::mpsc::UnboundedSender<OutgoingFrame>,
}

enum OutgoingFrame {
	Text(String),
	Close(u16, String),
}

impl NativeWebSocket {
	fn connect(url: &str, headers: &IndexMap<String, String>) -> PiResult<Rc<dyn WebSocket>> {
		use tokio_tungstenite::tungstenite::client::IntoClientRequest;

		let mut request = url
			.into_client_request()
			.map_err(|error| PiError::named("SyntaxError", format!("Failed to construct 'WebSocket': {error}")))?;
		for (name, value) in headers {
			let name = reqwest::header::HeaderName::from_bytes(name.as_bytes())
				.map_err(|error| PiError::type_error(error.to_string()))?;
			let value = reqwest::header::HeaderValue::from_str(value)
				.map_err(|error| PiError::type_error(error.to_string()))?;
			request.headers_mut().insert(name, value);
		}
		let (outgoing, receiver) = futures::channel::mpsc::unbounded();
		let socket = Rc::new(NativeWebSocket {
			state: Cell::new(CONNECTING),
			listeners: WebSocketListeners::default(),
			outgoing,
		});
		tokio::task::spawn_local(socket.clone().run(request, receiver));
		Ok(socket)
	}

	fn finish(&self, code: u16, reason: String, was_clean: bool) {
		self.state.set(CLOSED);
		self.listeners.dispatch(&WebSocketEvent::Close {
			code,
			reason,
			was_clean,
		});
	}

	fn fail(&self, message: String) {
		self.state.set(CLOSED);
		self.listeners
			.dispatch(&WebSocketEvent::Error { message: Some(message) });
		self.finish(1006, String::new(), false);
	}

	async fn run(
		self: Rc<Self>,
		request: tokio_tungstenite::tungstenite::handshake::client::Request,
		mut receiver: futures::channel::mpsc::UnboundedReceiver<OutgoingFrame>,
	) {
		use futures::SinkExt;
		use tokio_tungstenite::tungstenite::Message;
		use tokio_tungstenite::tungstenite::protocol::CloseFrame;
		use tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode;

		let connect = tokio_tungstenite::connect_async(request);
		futures::pin_mut!(connect);
		let stream = loop {
			futures::select_biased! {
				result = connect.as_mut().fuse() => break result,
				frame = receiver.next() => {
					if let Some(OutgoingFrame::Close(..)) | None = frame {
						// Closing a connecting socket fails the connection.
						self.finish(1006, String::new(), false);
						return;
					}
				}
			}
		};
		let mut stream = match stream {
			Ok((stream, _)) => stream,
			Err(error) => {
				self.fail(error.to_string());
				return;
			}
		};
		if self.state.get() == CONNECTING {
			self.state.set(OPEN);
		}
		self.listeners.dispatch(&WebSocketEvent::Open);
		loop {
			futures::select_biased! {
				frame = receiver.next() => match frame {
					Some(OutgoingFrame::Text(text)) => {
						if let Err(error) = stream.send(Message::text(text)).await {
							self.fail(error.to_string());
							return;
						}
					}
					Some(OutgoingFrame::Close(code, reason)) => {
						let frame = CloseFrame { code: CloseCode::from(code), reason: reason.into() };
						if let Err(error) = stream.send(Message::Close(Some(frame))).await {
							self.fail(error.to_string());
							return;
						}
					}
					None => {}
				},
				message = stream.next() => match message {
					Some(Ok(Message::Text(text))) => self.listeners.dispatch(&WebSocketEvent::Message {
						data: WebSocketData::Text(text.as_str().to_string()),
					}),
					Some(Ok(Message::Binary(bytes))) => self.listeners.dispatch(&WebSocketEvent::Message {
						data: WebSocketData::Binary(bytes.to_vec()),
					}),
					Some(Ok(Message::Close(frame))) => {
						let (code, reason) = frame
							.map(|frame| (u16::from(frame.code), frame.reason.as_str().to_string()))
							.unwrap_or((1005, String::new()));
						self.state.set(CLOSING);
						let _ = stream.close(None).await;
						self.finish(code, reason, true);
						return;
					}
					Some(Ok(_)) => {}
					Some(Err(error)) => {
						self.fail(error.to_string());
						return;
					}
					None => {
						self.finish(1006, String::new(), false);
						return;
					}
				},
			}
		}
	}
}

impl WebSocket for NativeWebSocket {
	fn ready_state(&self) -> Option<u16> {
		Some(self.state.get())
	}

	fn send(&self, data: &str) -> PiResult<()> {
		match self.state.get() {
			CONNECTING => Err(PiError::named("InvalidStateError", "Sent before connected.")),
			OPEN => {
				let _ = self.outgoing.unbounded_send(OutgoingFrame::Text(data.to_string()));
				Ok(())
			}
			_ => Ok(()),
		}
	}

	fn close(&self, code: u16, reason: &str) -> PiResult<()> {
		if matches!(self.state.get(), CLOSING | CLOSED) {
			return Ok(());
		}
		self.state.set(CLOSING);
		let _ = self
			.outgoing
			.unbounded_send(OutgoingFrame::Close(code, reason.to_string()));
		Ok(())
	}

	fn add_event_listener(&self, listener: WebSocketListener) -> WebSocketListenerId {
		self.listeners.add(listener)
	}

	fn remove_event_listener(&self, id: WebSocketListenerId) {
		self.listeners.remove(id);
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

#[cfg(test)]
mod tests {
	use std::cell::RefCell;

	use futures::{SinkExt, StreamExt};
	use tokio::io::{AsyncReadExt, AsyncWriteExt};

	use super::*;
	use crate::abort::AbortController;

	#[test]
	fn aborting_errors_the_body_with_the_signal_reason() {
		run(|| async {
			let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
			let url = format!("http://{}/", listener.local_addr().unwrap());
			tokio::task::spawn_local(async move {
				let (mut socket, _) = listener.accept().await.unwrap();
				let mut request = [0u8; 1024];
				let _ = socket.read(&mut request).await.unwrap();
				socket
					.write_all(b"HTTP/1.1 200 OK\r\ntransfer-encoding: chunked\r\n\r\n5\r\nfirst\r\n")
					.await
					.unwrap();
				tokio::time::sleep(std::time::Duration::from_secs(30)).await;
			});
			let controller = AbortController::new();
			let mut request = HttpRequest::new("GET", url);
			request.signal = Some(controller.signal());
			let mut response = crate::http::fetch(request, None).await.unwrap();
			let first = response.body.next().await.unwrap().unwrap();
			assert_eq!(&first[..], b"first");
			controller.abort();
			let error = response.body.next().await.unwrap().unwrap_err();
			assert_eq!(
				(error.name.as_str(), error.message.as_str()),
				("AbortError", "This operation was aborted")
			);
			assert!(response.body.next().await.is_none());
		});
	}

	#[test]
	fn websocket_opens_echoes_text_and_closes() {
		let events = run(|| async {
			let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
			let address = listener.local_addr().unwrap();
			tokio::task::spawn_local(async move {
				let (stream, _) = listener.accept().await.unwrap();
				let mut socket = tokio_tungstenite::accept_hdr_async(
					stream,
					|request: &tokio_tungstenite::tungstenite::handshake::server::Request,
					 response: tokio_tungstenite::tungstenite::handshake::server::Response| {
						assert_eq!(request.headers().get("x-test").unwrap(), "yes");
						Ok(response)
					},
				)
				.await
				.unwrap();
				while let Some(Ok(message)) = socket.next().await {
					if message.is_text() {
						socket.send(message).await.unwrap();
					} else if message.is_close() {
						let _ = socket.flush().await;
						break;
					}
				}
			});

			let headers = IndexMap::from([("x-test".to_string(), "yes".to_string())]);
			let socket = crate::websocket::connect(&format!("ws://{address}"), &headers)
				.unwrap()
				.unwrap();
			assert_eq!(socket.ready_state(), Some(CONNECTING));
			let events = Rc::new(RefCell::new(Vec::new()));
			let (done, finished) = futures::channel::oneshot::channel::<()>();
			let done = RefCell::new(Some(done));
			let recorded = events.clone();
			socket.add_event_listener(Rc::new(move |event: &WebSocketEvent| {
				recorded.borrow_mut().push(event.clone());
				if matches!(event, WebSocketEvent::Close { .. })
					&& let Some(done) = done.borrow_mut().take()
				{
					let _ = done.send(());
				}
			}));
			while socket.ready_state() != Some(OPEN) {
				tokio::time::sleep(std::time::Duration::from_millis(5)).await;
			}
			socket.send("hello").unwrap();
			while events.borrow().len() < 2 {
				tokio::time::sleep(std::time::Duration::from_millis(5)).await;
			}
			socket.close(1000, "done").unwrap();
			finished.await.unwrap();
			assert_eq!(socket.ready_state(), Some(CLOSED));
			events.borrow().clone()
		});
		assert_eq!(events[0], WebSocketEvent::Open);
		assert_eq!(
			events[1],
			WebSocketEvent::Message {
				data: WebSocketData::Text("hello".into())
			}
		);
		assert!(
			matches!(events[2], WebSocketEvent::Close { code: 1000, .. }),
			"{events:?}"
		);
	}

	#[test]
	fn websocket_connection_failure_dispatches_error_then_close() {
		let events = run(|| async {
			let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
			let address = listener.local_addr().unwrap();
			drop(listener);
			let socket = crate::websocket::connect(&format!("ws://{address}"), &IndexMap::new())
				.unwrap()
				.unwrap();
			let events = Rc::new(RefCell::new(Vec::new()));
			let recorded = events.clone();
			socket.add_event_listener(Rc::new(move |event: &WebSocketEvent| {
				recorded.borrow_mut().push(event.clone())
			}));
			while socket.ready_state() != Some(CLOSED) {
				tokio::time::sleep(std::time::Duration::from_millis(5)).await;
			}
			events.borrow().clone()
		});
		assert!(matches!(events[0], WebSocketEvent::Error { message: Some(_) }));
		assert_eq!(
			events[1],
			WebSocketEvent::Close {
				code: 1006,
				reason: String::new(),
				was_clean: false
			}
		);
	}
}
