// protocol/types: the MCP protocol types and the protocol version constants of the core.

import { wasm } from "@at-inc/pi-core";
import type * as ts from "@earendil-works/pi-mcp";

export type {
	CancelledNotification,
	ClientCapabilities,
	Implementation,
	InitializeParams,
	InitializeResult,
	ListResourcesResult,
	ListResourceTemplatesResult,
	ListToolsResult,
	ProgressNotification,
	ReadResourceResult,
	Resource,
	ResourceTemplate,
	Root,
	ServerCapabilities,
	SupportedProtocolVersion,
	Tool,
	ToolAnnotations,
	ToolExecution,
} from "@earendil-works/pi-mcp";

// The core's strings; the TS types are their literals.
export const LATEST_PROTOCOL_VERSION = wasm.mcpLatestProtocolVersion() as typeof ts.LATEST_PROTOCOL_VERSION;

export const SUPPORTED_PROTOCOL_VERSIONS = Object.freeze(
	wasm.mcpSupportedProtocolVersions(),
) as unknown as typeof ts.SUPPORTED_PROTOCOL_VERSIONS;
