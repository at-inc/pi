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
