//! Port of `packages/mcp/src/oauth/index.ts`.
//! PORT-STATUS: ported
//!
//! Re-exports the OAuth API. `OAuthCallbackServer` needs the `native` feature.

// <generated-modules>
pub mod callback;
pub mod discovery;
pub mod errors;
pub mod flow;
pub mod provider;
pub mod types;
// </generated-modules>

#[cfg(feature = "native")]
pub use callback::{OAuthCallback, OAuthCallbackPage, OAuthCallbackServer, OAuthCallbackServerOptions};
pub use discovery::{
	build_authorization_server_discovery_urls, discover_authorization_server_metadata, discover_oauth_server_info,
	discover_protected_resource_metadata, parse_www_authenticate, resource_url_from_server_url, select_resource,
};
pub use errors::{
	McpOAuthAuthorizationRequiredError, OAuthError, OAuthInsecureEndpointError, OAuthIssuerMismatchError,
	OAuthRegistrationError,
};
pub use flow::{
	AddClientAuthentication, OAuthClientMetadataDocument, OAuthClientProvider, OAuthFlowOptions, OAuthFlowResult,
	TokenRequestOptions, adapt_oauth_provider, authorize_mcp, exchange_authorization_code, refresh_authorization,
	register_client, start_authorization, step_up_scope,
};
pub use provider::{
	McpOAuthProvider, McpOAuthProviderOptions, McpOAuthState, McpOAuthStateStore, MemoryOAuthStateStore,
};
pub use types::{
	AuthorizationServerMetadata, OAuthChallenge, OAuthClientInformation, OAuthClientInformationFull,
	OAuthClientInformationMixed, OAuthClientMetadata, OAuthDiscoveryState, OAuthProtectedResourceMetadata,
	OAuthServerInfo, OAuthTokens,
};
