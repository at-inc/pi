//! Local HTTP servers: the counterpart of `http.createServer(handler).listen(port, host)`, as the OAuth callback
//! servers use it. A handler sees the request line and headers and answers with a status, headers and a text body;
//! request bodies are not read. Hosts without a listener (browsers, Workers) fail `listen_http`.

use std::rc::Rc;

use indexmap::IndexMap;

use crate::LocalBoxFuture;
use crate::error::{PiError, PiResult};

/// A request the server received. `url` is the raw request target (`/callback?code=...`), as `request.url` in
/// Node; header names are lowercased.
#[derive(Clone, Debug, Default)]
pub struct ServerRequest {
	pub method: String,
	pub url: String,
	pub headers: IndexMap<String, String>,
}

/// The answer to a request: `response.writeHead(status, headers).end(body)`.
#[derive(Clone, Debug, Default)]
pub struct ServerResponse {
	pub status: u16,
	pub headers: IndexMap<String, String>,
	pub body: String,
}

/// Answers one request.
pub type ServerHandler = Rc<dyn Fn(ServerRequest) -> LocalBoxFuture<'static, ServerResponse>>;

/// A listening server.
pub trait HttpServer {
	/// The bound port, `server.address().port`.
	fn port(&self) -> u16;
	/// `server.close()`: stops accepting connections.
	fn close(&self) -> LocalBoxFuture<'static, PiResult<()>>;
}

/// The error of `listen_http` on hosts without a listener.
pub fn listen_unsupported() -> PiError {
	PiError::new("HTTP servers are not supported on this host")
		.with_data(serde_json::json!({ "code": "ERR_METHOD_NOT_IMPLEMENTED" }))
}

/// Listens on `host:port` (port 0 picks a free one) on the current host. Fails like the server's `error` event, with
/// Node's `listen <code>: <description> <host>:<port>` message and `data.code`.
pub fn listen_http(
	host: &str,
	port: u16,
	handler: ServerHandler,
) -> LocalBoxFuture<'static, PiResult<Rc<dyn HttpServer>>> {
	crate::host::current().listen_http(host, port, handler)
}
