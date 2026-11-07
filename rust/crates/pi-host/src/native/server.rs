//! The native host's HTTP listener (`Host::listen_http`): a tokio `TcpListener` with the subset of `node:http` the
//! loopback OAuth servers use. It reads one request head per connection, within Node's `headersTimeout` of 60
//! seconds, and answers a malformed request line (unknown or lowercase method, a target byte outside visible ASCII,
//! bare LF line endings) with a bare 400 and a head over Node's 16 KiB limit with a bare 431, as Node's parser does.
//! The response differs from Node's in bytes: `content-length` instead of chunked encoding, `connection: close`
//! instead of keep-alive, no `Date` header, and a request body is never read. A HEAD request gets the headers only.
//! Like Node, the server keeps listening after an accept error: an aborted connection or descriptor exhaustion is
//! retried silently, other errors go to `on_error`. An error kind Node names but Rust does not is `UNKNOWN`.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::future::poll_fn;
use std::io::ErrorKind;
use std::net::SocketAddr;
use std::rc::Rc;
use std::task::Poll;

use futures::future::{AbortHandle, Abortable, Either, select};
use serde_json::json;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

use crate::error::{PiError, PiResult};
use crate::server::{HttpServer, HttpServerErrorHandler, HttpServerHandler, HttpServerRequest, HttpServerResponse};

/// A listening server; dropping it leaves it listening, like an unreferenced Node server.
struct NativeHttpServer {
	port: u16,
	socket: Rc<RefCell<Option<TcpListener>>>,
	accepting: AbortHandle,
	connections: Rc<RefCell<HashMap<u64, AbortHandle>>>,
}

impl HttpServer for NativeHttpServer {
	fn port(&self) -> u16 {
		self.port
	}

	fn close(&self) {
		self.socket.borrow_mut().take();
		self.accepting.abort();
	}

	fn close_all_connections(&self) {
		for (_, connection) in self.connections.borrow_mut().drain() {
			connection.abort();
		}
	}
}

/// `createServer(handler).listen(port, host)`: binds the first address the host resolves to.
pub(super) async fn listen(
	host: String,
	port: u16,
	handler: HttpServerHandler,
	on_error: HttpServerErrorHandler,
) -> PiResult<Rc<dyn HttpServer>> {
	let address = tokio::net::lookup_host((host.as_str(), port))
		.await
		.ok()
		.and_then(|mut addresses| addresses.next());
	let Some(address) = address else {
		return Err(PiError::new(format!("getaddrinfo ENOTFOUND {host}")).with_data(json!({
			"errno": -3008,
			"code": "ENOTFOUND",
			"syscall": "getaddrinfo",
			"hostname": host,
		})));
	};
	let listener = TcpListener::bind(address)
		.await
		.map_err(|error| listen_error(&error, address))?;
	let port = listener
		.local_addr()
		.map_err(|_| PiError::new("OAuth callback server did not bind to TCP"))?
		.port();
	let socket = Rc::new(RefCell::new(Some(listener)));
	let connections = Rc::new(RefCell::new(HashMap::new()));
	let (accepting, registration) = AbortHandle::new_pair();
	crate::host::spawn_local({
		let accept = accept_connections(socket.clone(), connections.clone(), handler, on_error);
		async move {
			let _ = Abortable::new(accept, registration).await;
		}
	});
	Ok(Rc::new(NativeHttpServer {
		port,
		socket,
		accepting,
		connections,
	}))
}

fn listen_error(error: &std::io::Error, address: SocketAddr) -> PiError {
	let (code, description) = uv_error(error);
	PiError::new(format!(
		"listen {code}: {description} {}:{}",
		address.ip(),
		address.port()
	))
	.with_data(json!({
		"errno": error.raw_os_error().map(|errno| -errno),
		"code": code,
		"syscall": "listen",
		"address": address.ip().to_string(),
		"port": address.port(),
	}))
}

/// libuv's code and description for the errors a listening socket reports.
fn uv_error(error: &std::io::Error) -> (&'static str, &'static str) {
	match error.kind() {
		ErrorKind::AddrInUse => ("EADDRINUSE", "address already in use"),
		ErrorKind::AddrNotAvailable => ("EADDRNOTAVAIL", "address not available"),
		ErrorKind::PermissionDenied => ("EACCES", "permission denied"),
		ErrorKind::InvalidInput => ("EINVAL", "invalid argument"),
		ErrorKind::ConnectionAborted => ("ECONNABORTED", "software caused connection abort"),
		ErrorKind::ConnectionReset => ("ECONNRESET", "connection reset by peer"),
		_ => ("UNKNOWN", "unknown error"),
	}
}

const EMFILE: i32 = 24;
const ENFILE: i32 = 23;
const DESCRIPTOR_RETRY_MS: f64 = 100.0;

async fn accept_connections(
	socket: Rc<RefCell<Option<TcpListener>>>,
	connections: Rc<RefCell<HashMap<u64, AbortHandle>>>,
	handler: HttpServerHandler,
	on_error: HttpServerErrorHandler,
) {
	let next_id = Cell::new(0u64);
	loop {
		let accepted = poll_fn(|cx| match socket.borrow().as_ref() {
			Some(listener) => listener.poll_accept(cx).map(Some),
			None => Poll::Ready(None),
		})
		.await;
		match accepted {
			None => return,
			Some(Ok((stream, _))) => {
				let id = next_id.get();
				next_id.set(id + 1);
				let (connection, registration) = AbortHandle::new_pair();
				connections.borrow_mut().insert(id, connection);
				let handler = handler.clone();
				let connections = connections.clone();
				crate::host::spawn_local(async move {
					let _ = Abortable::new(serve(&handler, stream), registration).await;
					connections.borrow_mut().remove(&id);
				});
			}
			// libuv retries an aborted connection and waits out descriptor exhaustion without reporting either.
			Some(Err(error)) if error.kind() == ErrorKind::ConnectionAborted => {}
			Some(Err(error)) if matches!(error.raw_os_error(), Some(EMFILE | ENFILE)) => {
				let _ = crate::host::sleep(DESCRIPTOR_RETRY_MS, None).await;
			}
			// Node emits `error` on the server and keeps listening.
			Some(Err(error)) => {
				let (code, _) = uv_error(&error);
				on_error(PiError::new(format!("accept {code}")).with_data(json!({
					"errno": error.raw_os_error().map(|errno| -errno),
					"code": code,
					"syscall": "accept",
				})));
			}
		}
	}
}

/// Node's `maxHeaderSize`.
const MAX_REQUEST_HEAD_BYTES: usize = 16 * 1024;
/// Node's default `headersTimeout`.
const HEADERS_TIMEOUT_MS: f64 = 60_000.0;

enum RequestHead {
	Complete(Vec<u8>),
	/// A line ends in a bare LF, which Node's parser rejects.
	Malformed,
	TooLarge,
	Closed,
}

async fn read_head(stream: &mut TcpStream) -> RequestHead {
	let mut head = Vec::new();
	let mut chunk = [0u8; 4096];
	loop {
		match stream.read(&mut chunk).await {
			Ok(0) | Err(_) => return RequestHead::Closed,
			Ok(read) => head.extend_from_slice(&chunk[..read]),
		}
		let end = head.windows(4).position(|window| window == b"\r\n\r\n");
		let scanned = &head[..end.map_or(head.len(), |end| end + 4)];
		if scanned
			.iter()
			.enumerate()
			.any(|(index, byte)| *byte == b'\n' && (index == 0 || scanned[index - 1] != b'\r'))
		{
			return RequestHead::Malformed;
		}
		if scanned.len() > MAX_REQUEST_HEAD_BYTES {
			return RequestHead::TooLarge;
		}
		if let Some(end) = end {
			head.truncate(end + 4);
			return RequestHead::Complete(head);
		}
	}
}

/// The methods Node's HTTP parser accepts.
const METHODS: &[&str] = &[
	"DELETE",
	"GET",
	"HEAD",
	"POST",
	"PUT",
	"CONNECT",
	"OPTIONS",
	"TRACE",
	"COPY",
	"LOCK",
	"MKCOL",
	"MOVE",
	"PROPFIND",
	"PROPPATCH",
	"SEARCH",
	"UNLOCK",
	"BIND",
	"REBIND",
	"UNBIND",
	"ACL",
	"REPORT",
	"MKACTIVITY",
	"CHECKOUT",
	"MERGE",
	"M-SEARCH",
	"NOTIFY",
	"SUBSCRIBE",
	"UNSUBSCRIBE",
	"PATCH",
	"PURGE",
	"MKCALENDAR",
	"LINK",
	"UNLINK",
	"SOURCE",
	"QUERY",
];

/// The request line, when Node's parser accepts it.
fn request_line(head: &[u8]) -> Option<HttpServerRequest> {
	let line = &head[..head.windows(2).position(|window| window == b"\r\n")?];
	let mut parts = line.split(|byte| *byte == b' ');
	let method = std::str::from_utf8(parts.next()?).ok()?;
	let target = parts.next()?;
	let version = parts.next()?;
	if parts.next().is_some()
		|| !METHODS.contains(&method)
		|| target.is_empty()
		|| !target.iter().all(|byte| (0x21..=0x7e).contains(byte))
		|| !version.starts_with(b"HTTP/1.")
	{
		return None;
	}
	Some(HttpServerRequest {
		method: method.to_string(),
		target: String::from_utf8(target.to_vec()).ok()?,
	})
}

/// The bare response Node's parser sends for a request it rejects.
async fn reject(stream: &mut TcpStream, status: &str) {
	let _ = stream
		.write_all(format!("HTTP/1.1 {status}\r\nConnection: close\r\n\r\n").as_bytes())
		.await;
	let _ = stream.shutdown().await;
}

async fn respond(stream: &mut TcpStream, method: &str, response: HttpServerResponse) {
	let reason = match response.status {
		200 => "OK",
		400 => "Bad Request",
		404 => "Not Found",
		409 => "Conflict",
		500 => "Internal Server Error",
		_ => "Bad Gateway",
	};
	let mut bytes = format!("HTTP/1.1 {} {reason}\r\n", response.status);
	for (name, value) in &response.headers {
		bytes.push_str(&format!("{name}: {value}\r\n"));
	}
	bytes.push_str(&format!(
		"content-length: {}\r\nconnection: close\r\n\r\n",
		response.body.len()
	));
	if method != "HEAD" {
		bytes.push_str(&response.body);
	}
	let _ = stream.write_all(bytes.as_bytes()).await;
	let _ = stream.shutdown().await;
}

async fn serve(handler: &HttpServerHandler, mut stream: TcpStream) {
	let head = {
		let read = read_head(&mut stream);
		let deadline = crate::host::sleep(HEADERS_TIMEOUT_MS, None);
		futures::pin_mut!(read, deadline);
		match select(read, deadline).await {
			Either::Left((head, _)) => head,
			Either::Right(_) => return,
		}
	};
	let head = match head {
		RequestHead::Complete(head) => head,
		RequestHead::Malformed => return reject(&mut stream, "400 Bad Request").await,
		RequestHead::TooLarge => return reject(&mut stream, "431 Request Header Fields Too Large").await,
		RequestHead::Closed => return,
	};
	let Some(request) = request_line(&head) else {
		return reject(&mut stream, "400 Bad Request").await;
	};
	let method = request.method.clone();
	let response = handler(request).await;
	respond(&mut stream, &method, response).await;
}
