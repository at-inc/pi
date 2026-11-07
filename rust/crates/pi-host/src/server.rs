//! HTTP listener capability, the counterpart of `http.createServer(handler).listen(port, host)` that the loopback
//! OAuth callback servers use: one request head per connection, answered by a handler with a status, headers and a
//! text body (`response.writeHead(status, headers)` plus `response.end(body)`).
//!
//! The host binds like Node: the first address the host name resolves to, failing with Node's listen error
//! (`listen EADDRINUSE: address already in use 127.0.0.1:1455`, `data` with `errno`, `code`, `syscall`, `address`,
//! `port`) instead of picking another port. After it listens, an error of the server goes to `on_error` (Node's
//! `error` event) and the server keeps listening. A server stays open until `close()`, also when its handle is
//! dropped, like an unreferenced Node server.

use std::rc::Rc;

use crate::LocalBoxFuture;
use crate::error::{PiError, PiResult};

/// Method and raw target (`request.url`) of a request.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HttpServerRequest {
	pub method: String,
	pub target: String,
}

/// `response.writeHead(status, headers)` plus `response.end(body)`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HttpServerResponse {
	pub status: u16,
	pub headers: Vec<(String, String)>,
	pub body: String,
}

/// The request handler of `createServer`.
pub type HttpServerHandler = Rc<dyn Fn(HttpServerRequest) -> LocalBoxFuture<'static, HttpServerResponse>>;
/// The server's `error` event after it listens.
pub type HttpServerErrorHandler = Rc<dyn Fn(PiError)>;

/// A listening server.
pub trait HttpServer {
	/// The bound port (`server.address().port`).
	fn port(&self) -> u16;
	/// `server.close()`: stops accepting new connections; open connections finish their response.
	fn close(&self);
	/// `server.closeAllConnections()`.
	fn close_all_connections(&self);
}

/// The rejection of hosts without an HTTP listener.
pub fn listen_unsupported() -> PiError {
	PiError::new("HTTP servers are not supported on this host")
}

/// `http.createServer(handler).listen(port, host)` on the current host. Resolves once the server listens.
pub async fn listen_http(
	host: &str,
	port: u16,
	handler: HttpServerHandler,
	on_error: HttpServerErrorHandler,
) -> PiResult<Rc<dyn HttpServer>> {
	crate::host::current().listen_http(host, port, handler, on_error).await
}
