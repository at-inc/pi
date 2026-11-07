//! Port of `packages/mcp/src/index.ts`.
//! PORT-STATUS: reviewed
//!
//! Re-exports the public API. TS type-only exports map to the Rust types of the same names.

// <generated-modules>
pub mod auth_provider;
pub mod client;
pub mod oauth;
pub mod protocol;
pub mod testing;
pub mod transports;
// </generated-modules>

pub use auth_provider::{AuthProvider, McpFetch, UnauthorizedContext};
pub use client::{McpClient, McpClientOptions, McpRequestOptions};
pub use protocol::content::{
	AudioContent, BlobResourceContents, CallToolResult, ContentAnnotations, ContentBlock, EmbeddedResourceContent,
	ImageContent, LlmContent, ResourceLinkContent, TextContent, TextResourceContents, to_llm_content,
};
pub use protocol::jsonrpc::{
	JSON_RPC_ERROR_CODES, JsonRpcErrorObject, JsonRpcErrorResponse, JsonRpcId, JsonRpcMessage, JsonRpcNotification,
	JsonRpcRequest, JsonRpcResponse, JsonRpcSuccessResponse, McpAbortError, McpConnectionClosedError, McpError,
	McpTimeoutError, is_json_rpc_notification, is_json_rpc_request, is_json_rpc_response, parse_json_rpc_message,
};
pub use protocol::types::{
	CancelledNotification, ClientCapabilities, Implementation, InitializeParams, InitializeResult,
	LATEST_PROTOCOL_VERSION, ListResourceTemplatesResult, ListResourcesResult, ListToolsResult, ProgressNotification,
	ReadResourceResult, Resource, ResourceTemplate, Root, SUPPORTED_PROTOCOL_VERSIONS, ServerCapabilities,
	SupportedProtocolVersion, Tool, ToolAnnotations, ToolExecution,
};
pub use transports::stdio::{StdioTransport, StdioTransportOptions};
pub use transports::streamable_http::{
	McpAuthRequiredError, McpHttpError, McpSessionExpiredError, StreamableHttpTransport, StreamableHttpTransportOptions,
};
pub use transports::transport::{
	McpTransport, McpTransportCloseListener, McpTransportErrorListener, McpTransportMessageListener,
	McpTransportSendOptions,
};
