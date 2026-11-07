// protocol/content: the content types and toLlmContent over the Rust conversion.

import { wasm } from "@at-inc/pi-core";
import type * as ts from "@earendil-works/pi-mcp";
import { attempt } from "../internal/errors.ts";

export type {
	AudioContent,
	BlobResourceContents,
	CallToolResult,
	ContentAnnotations,
	ContentBlock,
	EmbeddedResourceContent,
	ImageContent,
	LlmContent,
	ResourceLinkContent,
	TextContent,
	TextResourceContents,
} from "@earendil-works/pi-mcp";

export const toLlmContent: typeof ts.toLlmContent = (result) => attempt(() => wasm.mcpToLlmContent(result));
