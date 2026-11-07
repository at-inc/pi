// protocol/jsonrpc: the JSON-RPC message types and checks over the Rust ones, and the error classes.

import { wasm } from "@at-inc/pi-core";
import type * as ts from "@earendil-works/pi-mcp";
import { attempt } from "../internal/errors.ts";

export type {
	JsonRpcErrorObject,
	JsonRpcErrorResponse,
	JsonRpcId,
	JsonRpcMessage,
	JsonRpcNotification,
	JsonRpcRequest,
	JsonRpcResponse,
	JsonRpcSuccessResponse,
} from "@earendil-works/pi-mcp";
export { McpAbortError, McpConnectionClosedError, McpError, McpTimeoutError } from "../internal/errors.ts";

// The core's object of the same codes; the TS type is the literal `as const` object.
export const JSON_RPC_ERROR_CODES = Object.freeze(wasm.mcpJsonRpcErrorCodes()) as typeof ts.JSON_RPC_ERROR_CODES;

export const isJsonRpcRequest: typeof ts.isJsonRpcRequest = (message): message is ts.JsonRpcRequest =>
	wasm.mcpIsJsonRpcRequest(message);

export const isJsonRpcNotification: typeof ts.isJsonRpcNotification = (message): message is ts.JsonRpcNotification =>
	wasm.mcpIsJsonRpcNotification(message);

export const isJsonRpcResponse: typeof ts.isJsonRpcResponse = (message): message is ts.JsonRpcResponse =>
	wasm.mcpIsJsonRpcResponse(message);

export const parseJsonRpcMessage: typeof ts.parseJsonRpcMessage = (value) => {
	attempt(() => wasm.mcpParseJsonRpcMessage(value));
	return value as ts.JsonRpcMessage;
};
