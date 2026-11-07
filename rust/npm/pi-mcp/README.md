# @at-inc/pi-mcp-rs

`@earendil-works/pi-mcp` backed by the pi Rust core. It has the same exports map (`.`, `./oauth`, `./testing`) and
module layout, the types of `@earendil-works/pi-mcp` and a runtime in wasm (`@at-inc/pi-core`). Its declaration files
re-export the TS package's, which it depends on as `@at-inc/pi-mcp-ts`, so the usual alias keeps working:
`"@earendil-works/pi-mcp": "npm:@at-inc/pi-mcp-rs@<version>"`.

## Bridged

Every runtime export of the TS package:

- `.`: `McpClient`, `StdioTransport`, `StreamableHttpTransport`, `toLlmContent`, `JSON_RPC_ERROR_CODES`,
  `isJsonRpcRequest`, `isJsonRpcNotification`, `isJsonRpcResponse`, `parseJsonRpcMessage`, `LATEST_PROTOCOL_VERSION`,
  `SUPPORTED_PROTOCOL_VERSIONS`, and the error classes `McpError`, `McpConnectionClosedError`, `McpTimeoutError`,
  `McpAbortError`, `McpHttpError`, `McpAuthRequiredError`, `McpSessionExpiredError`.
- `./oauth`: `authorizeMcp`, `adaptOAuthProvider`, `startAuthorization`, `registerClient`,
  `exchangeAuthorizationCode`, `refreshAuthorization`, `stepUpScope`, `McpOAuthProvider`, `MemoryOAuthStateStore`,
  `OAuthCallbackServer`, the discovery functions, and the error classes `OAuthError`, `OAuthIssuerMismatchError`,
  `OAuthInsecureEndpointError`, `OAuthRegistrationError`, `McpOAuthAuthorizationRequiredError`.
- `./testing`: `InMemoryTransport`, `createInMemoryTransportPair`.
- `transports/streamable-http`'s `consumeSseStream`, which the TS tests import directly.

Errors thrown by the core are instances of the facade's error classes, so `instanceof` works. A facade transport
connected to `McpClient` passes its Rust transport to the Rust client; a transport written in JS is called through
its methods. `StdioTransport` and `OAuthCallbackServer` need the host's processes and HTTP listener: the Node host
has them, the Workers host fails them with `code: "ERR_METHOD_NOT_IMPLEMENTED"`.

## Differences from the TS

- An error a JS callback throws (a request handler, a JS transport, an auth or OAuth provider) crosses the Rust
  core as its `name`, `message` and plain-data fields. The caller gets an error of the same class with those
  fields, not the same object.
- `isJsonRpcRequest`, `isJsonRpcNotification` and `isJsonRpcResponse` check the value as JSON data, so a value that
  is not JSON data (a function, a cycle) is not a message.
- `new McpClient({ protocolVersion })` throws a `TypeError` for a version outside `SUPPORTED_PROTOCOL_VERSIONS`,
  which the TS type already excludes.
- `OAuthCallbackServer` answers a request target that is not a valid URL with 404; the TS handler throws.

## Tests

`npm run test:vitest` runs the TS package's own tests (`packages/mcp/test`) against the facade, and
`npm run test:workers` an MCP session on workerd (after `npm run build:wasm` in `rust/npm`).
