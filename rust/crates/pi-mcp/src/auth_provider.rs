//! Port of `packages/mcp/src/auth-provider.ts`.
//! PORT-STATUS: ported
//!
//! `AuthProvider` is an interface with an optional method in TS; here it is a struct of callbacks, so
//! `onUnauthorized` can be absent and providers are written the way the TS object literals are.

use std::rc::Rc;

use pi_host::{FetchFunction, HttpResponse, LocalBoxFuture, PiResult};
use url::Url;

/// `typeof fetch` as MCP transports call it.
pub type McpFetch = FetchFunction;

pub struct UnauthorizedContext {
	/// The 401 response, or a 403 response whose challenge reports `insufficient_scope`.
	pub response: HttpResponse,
	pub server_url: Url,
	pub fetch: McpFetch,
	/// Access token the rejected request carried, if any. A different current token means another request already refreshed it.
	pub token: Option<String>,
}

/// `token()`.
pub type AuthTokenFunction = Rc<dyn Fn() -> LocalBoxFuture<'static, PiResult<Option<String>>>>;
/// `onUnauthorized(context)`.
pub type OnUnauthorizedFunction = Rc<dyn Fn(UnauthorizedContext) -> LocalBoxFuture<'static, PiResult<()>>>;

/// Supplies bearer tokens to an MCP HTTP transport and may refresh them after a 401 response.
#[derive(Clone)]
pub struct AuthProvider {
	/// Token for the next request. May refresh it over the network first, so the transport calls it only
	/// before requests whose answer it waits for. Closing a session reuses the token of the last request.
	pub token: AuthTokenFunction,
	pub on_unauthorized: Option<OnUnauthorizedFunction>,
}
