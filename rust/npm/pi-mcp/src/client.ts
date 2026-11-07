// client: McpClient over the Rust client. A facade transport is connected by its Rust handle; a transport written
// in JS is called through its methods.

import { wasm } from "@at-inc/pi-core";
import type { McpClientHandle } from "@at-inc/pi-core/wasm";
import type {
	CallToolResult,
	Implementation,
	InitializeResult,
	ListResourcesResult,
	ListResourceTemplatesResult,
	McpClientOptions,
	McpRequestOptions,
	McpTransport,
	ReadResourceResult,
	Resource,
	ResourceTemplate,
	ServerCapabilities,
	Tool,
} from "@earendil-works/pi-mcp";
import { attempt, errorListener, settle } from "./internal/errors.ts";
import { transportHandle } from "./internal/native-transport.ts";

export type { McpClientOptions, McpRequestOptions } from "@earendil-works/pi-mcp";

type RequestHandler = (params: unknown, context: { signal: AbortSignal }) => unknown | Promise<unknown>;

/** `McpClient`: an MCP client session over one transport. */
export class McpClient {
	readonly options: Readonly<McpClientOptions>;
	private handle: McpClientHandle;

	constructor(options: McpClientOptions) {
		this.options = Object.freeze({ ...options });
		this.handle = attempt(() => wasm.mcpCreateClient(options));
	}

	get connectionState(): "idle" | "connecting" | "connected" | "closed" {
		return this.handle.connectionState() as McpClient["connectionState"];
	}

	get serverInfo(): Implementation | undefined {
		return this.handle.serverInfo();
	}

	get serverCapabilities(): ServerCapabilities | undefined {
		return this.handle.serverCapabilities();
	}

	get instructions(): string | undefined {
		return this.handle.instructions();
	}

	get protocolVersion(): string | undefined {
		return this.handle.protocolVersion();
	}

	connect(transport: McpTransport): Promise<InitializeResult> {
		const native = transportHandle(transport);
		return settle(this.handle.connect(native?.share(), native ? undefined : transport));
	}

	request<Result = unknown>(
		method: string,
		params?: Record<string, unknown>,
		options: McpRequestOptions = {},
	): Promise<Result> {
		return settle(this.handle.request(method, params, options));
	}

	notify(method: string, params?: Record<string, unknown>): Promise<void> {
		return settle(this.handle.notify(method, params));
	}

	setRequestHandler(method: string, handler: RequestHandler): () => void {
		return this.handle.setRequestHandler(method, handler);
	}

	onNotification(method: string, listener: (params: unknown) => void): () => void {
		return this.handle.onNotification(method, listener);
	}

	onError(listener: (error: Error) => void): () => void {
		return this.handle.onError(errorListener(listener));
	}

	/** Called once when the connection closes, whether the transport dropped or `close()` was called. */
	onClose(listener: () => void): () => void {
		return this.handle.onClose(listener);
	}

	ping(options: McpRequestOptions = {}): Promise<void> {
		return settle(this.handle.ping(options));
	}

	listTools(options: McpRequestOptions = {}): Promise<Tool[]> {
		return settle(this.handle.listTools(options));
	}

	/** Every resource, following `nextCursor` through all pages. */
	listResources(options: McpRequestOptions = {}): Promise<Resource[]> {
		return settle(this.handle.listResources(options));
	}

	/** One page of resources, starting at `cursor`. */
	listResourcesPage(cursor?: string, options: McpRequestOptions = {}): Promise<ListResourcesResult> {
		return settle(this.handle.listResourcesPage(cursor, options));
	}

	/** Every resource template, following `nextCursor` through all pages. */
	listResourceTemplates(options: McpRequestOptions = {}): Promise<ResourceTemplate[]> {
		return settle(this.handle.listResourceTemplates(options));
	}

	/** One page of resource templates, starting at `cursor`. */
	listResourceTemplatesPage(cursor?: string, options: McpRequestOptions = {}): Promise<ListResourceTemplatesResult> {
		return settle(this.handle.listResourceTemplatesPage(cursor, options));
	}

	readResource(uri: string, options: McpRequestOptions = {}): Promise<ReadResourceResult> {
		return settle(this.handle.readResource(uri, options));
	}

	callTool(name: string, args?: Record<string, unknown>, options: McpRequestOptions = {}): Promise<CallToolResult> {
		return settle(this.handle.callTool(name, args, options));
	}

	close(): Promise<void> {
		return settle(this.handle.close());
	}
}
