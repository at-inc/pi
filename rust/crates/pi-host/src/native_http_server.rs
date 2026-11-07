//! HTTP servers of the native host: a tokio `TcpListener` that reads one request head per connection and answers
//! with `connection: close`, instead of `node:http`. A request head must arrive within 60 seconds, like Node's
//! `headersTimeout`. `close` stops accepting and drops open connections at once instead of waiting for them.

use std::cell::RefCell;
use std::rc::Rc;

use futures::future::{AbortHandle, Abortable, Either, select};
use indexmap::IndexMap;
use serde_json::json;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

use crate::LocalBoxFuture;
use crate::error::{PiError, PiResult};
use crate::http_server::{HttpServer, ServerHandler, ServerRequest};

const MAX_REQUEST_HEAD_BYTES: usize = 64 * 1024;
/// Node's default `headersTimeout`.
const HEADERS_TIMEOUT_MS: f64 = 60_000.0;

/// Binds `host:port` and serves every connection with `handler` on a local task.
pub(crate) async fn listen(host: String, port: u16, handler: ServerHandler) -> PiResult<Rc<dyn HttpServer>> {
	let listener = TcpListener::bind((host.as_str(), port))
		.await
		.map_err(|error| listen_error(&error, &host, port))?;
	let port = listener
		.local_addr()
		.map_err(|error| listen_error(&error, &host, port))?
		.port();
	let (accepting, registration) = AbortHandle::new_pair();
	let server = Rc::new(NativeHttpServer {
		port,
		accepting,
		connections: Rc::new(RefCell::new(Vec::new())),
	});
	let connections = server.connections.clone();
	tokio::task::spawn_local(Abortable::new(
		async move {
			while let Ok((stream, _)) = listener.accept().await {
				let (connection, registration) = AbortHandle::new_pair();
				connections.borrow_mut().retain(|connection| !connection.is_aborted());
				connections.borrow_mut().push(connection.clone());
				let handler = handler.clone();
				tokio::task::spawn_local(async move {
					let _ = Abortable::new(serve(handler, stream), registration).await;
					connection.abort();
				});
			}
		},
		registration,
	));
	Ok(server)
}

struct NativeHttpServer {
	port: u16,
	accepting: AbortHandle,
	connections: Rc<RefCell<Vec<AbortHandle>>>,
}

impl HttpServer for NativeHttpServer {
	fn port(&self) -> u16 {
		self.port
	}

	fn close(&self) -> LocalBoxFuture<'static, PiResult<()>> {
		self.accepting.abort();
		for connection in std::mem::take(&mut *self.connections.borrow_mut()) {
			connection.abort();
		}
		Box::pin(async { Ok(()) })
	}
}

fn listen_error(error: &std::io::Error, host: &str, port: u16) -> PiError {
	let (code, description) = match error.kind() {
		std::io::ErrorKind::AddrInUse => ("EADDRINUSE", "address already in use"),
		std::io::ErrorKind::AddrNotAvailable => ("EADDRNOTAVAIL", "address not available"),
		std::io::ErrorKind::PermissionDenied => ("EACCES", "permission denied"),
		_ => ("EIO", "i/o error"),
	};
	PiError::new(format!("listen {code}: {description} {host}:{port}")).with_data(json!({ "code": code }))
}

async fn read_head(stream: &mut TcpStream) -> Option<Vec<u8>> {
	let mut head = Vec::new();
	let mut chunk = [0u8; 4096];
	while !head.windows(4).any(|window| window == b"\r\n\r\n") {
		match stream.read(&mut chunk).await {
			Ok(0) | Err(_) => return None,
			Ok(read) => head.extend_from_slice(&chunk[..read]),
		}
		if head.len() > MAX_REQUEST_HEAD_BYTES {
			return None;
		}
	}
	Some(head)
}

/// The method, target and lowercased headers of a request head.
fn parse_head(head: &str) -> ServerRequest {
	let mut lines = head.split("\r\n");
	let mut request_line = lines.next().unwrap_or("").split_whitespace();
	let method = request_line.next().unwrap_or("GET").to_string();
	let url = request_line.next().unwrap_or("/").to_string();
	let mut headers: IndexMap<String, String> = IndexMap::new();
	for line in lines {
		let Some((name, value)) = line.split_once(':') else {
			continue;
		};
		headers
			.entry(name.trim().to_ascii_lowercase())
			.and_modify(|existing| {
				existing.push_str(", ");
				existing.push_str(value.trim());
			})
			.or_insert_with(|| value.trim().to_string());
	}
	ServerRequest { method, url, headers }
}

async fn serve(handler: ServerHandler, mut stream: TcpStream) {
	let head = {
		let read = read_head(&mut stream);
		let deadline = crate::host::sleep(HEADERS_TIMEOUT_MS, None);
		futures::pin_mut!(read, deadline);
		match select(read, deadline).await {
			Either::Left((Some(head), _)) => head,
			_ => return,
		}
	};
	let response = handler(parse_head(&String::from_utf8_lossy(&head))).await;
	let reason = reqwest::StatusCode::from_u16(response.status)
		.ok()
		.and_then(|status| status.canonical_reason())
		.unwrap_or("");
	let mut out = format!("HTTP/1.1 {} {reason}\r\n", response.status);
	for (name, value) in &response.headers {
		out.push_str(&format!("{name}: {value}\r\n"));
	}
	out.push_str(&format!(
		"content-length: {}\r\nconnection: close\r\n\r\n",
		response.body.len()
	));
	out.push_str(&response.body);
	let _ = stream.write_all(out.as_bytes()).await;
	let _ = stream.shutdown().await;
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::http::{HttpRequest, fetch};
	use crate::http_server::{ServerResponse, listen_http};
	use crate::native::run;

	#[test]
	fn answers_requests_with_the_handler_response_until_closed() {
		run(|| async {
			let seen = Rc::new(RefCell::new(Vec::new()));
			let requests = seen.clone();
			let handler: ServerHandler = Rc::new(move |request: ServerRequest| {
				requests
					.borrow_mut()
					.push((request.method, request.url, request.headers.get("x-test").cloned()));
				Box::pin(async {
					ServerResponse {
						status: 201,
						headers: IndexMap::from([("content-type".into(), "text/plain".into())]),
						body: "created".into(),
					}
				})
			});
			let server = listen_http("127.0.0.1", 0, handler).await.unwrap();
			let url = format!("http://127.0.0.1:{}/callback?code=1", server.port());
			let mut request = HttpRequest::new("GET", url.clone());
			request.headers.insert("x-test".into(), "yes".into());
			let response = fetch(request, None).await.unwrap();
			assert_eq!(response.status, 201);
			assert_eq!(
				response.headers.get("content-type").map(String::as_str),
				Some("text/plain")
			);
			assert_eq!(response.text().await.unwrap(), "created");
			assert_eq!(
				*seen.borrow(),
				vec![(
					"GET".to_string(),
					"/callback?code=1".to_string(),
					Some("yes".to_string())
				)]
			);
			server.close().await.unwrap();
			assert!(fetch(HttpRequest::new("GET", url), None).await.is_err());
		});
	}

	#[test]
	fn fails_like_the_error_event_when_the_port_is_taken() {
		run(|| async {
			let handler: ServerHandler = Rc::new(|_| Box::pin(async { ServerResponse::default() }));
			let server = listen_http("127.0.0.1", 0, handler.clone()).await.unwrap();
			let Err(error) = listen_http("127.0.0.1", server.port(), handler).await else {
				panic!("listening on a taken port succeeded");
			};
			assert_eq!(
				error.message,
				format!("listen EADDRINUSE: address already in use 127.0.0.1:{}", server.port())
			);
			assert_eq!(
				error.data.as_ref().and_then(|data| data.get("code")),
				Some(&json!("EADDRINUSE"))
			);
			server.close().await.unwrap();
		});
	}
}
