// transports/streamable-http: StreamableHttpTransport over the Rust transport, whose requests go through
// `options.fetch` or the global fetch, and consumeSseStream over the Rust SSE parser.

import { wasm } from "@at-inc/pi-core";
import type { McpFetch, StreamableHttpTransportOptions } from "@earendil-works/pi-mcp";
import { authProviderHandle, jsAuthProvider } from "../internal/auth-provider.ts";
import { attempt, settle } from "../internal/errors.ts";
import { HANDLE, NativeTransport } from "../internal/native-transport.ts";

export type { StreamableHttpTransportOptions } from "@earendil-works/pi-mcp";
export { McpAuthRequiredError, McpHttpError, McpSessionExpiredError } from "../internal/errors.ts";

/** One server-sent event. */
export interface SseEvent {
	event?: string;
	data: string;
	id?: string;
}

/** Listeners and limits of `consumeSseStream`. */
export interface ConsumeSseOptions {
	maxEventBytes?: number;
	onEvent(event: SseEvent): void;
	/** Called for every `id` field, including events without data (for example resumption priming events). */
	onId?(id: string): void;
	/** Called for every valid `retry` field, in milliseconds. */
	onRetry?(delayMs: number): void;
}

/** `consumeSseStream(stream, options)`: parses a server-sent event stream and calls the listeners. */
export function consumeSseStream(stream: ReadableStream<Uint8Array>, options: ConsumeSseOptions): Promise<void> {
	return settle(wasm.mcpConsumeSseStream(stream, options));
}

/** `StreamableHttpTransport`: MCP over HTTP POST requests and server-sent event streams. */
export class StreamableHttpTransport extends NativeTransport {
	readonly url: URL;
	readonly options: Readonly<StreamableHttpTransportOptions>;

	constructor(options: StreamableHttpTransportOptions) {
		const fetch = options.fetch ?? globalThis.fetch;
		// Called without a receiver: Workers reject the platform fetch called as a method of another object.
		const unbound: McpFetch = (input, init) => fetch(input, init);
		const provider = options.authProvider;
		const handle = authProviderHandle(provider);
		super(
			attempt(() =>
				wasm.mcpCreateStreamableHttpTransport(
					{ ...options, fetch: unbound },
					handle,
					handle || !provider ? undefined : jsAuthProvider(provider, unbound),
				),
			),
		);
		this.options = Object.freeze({ ...options, headers: options.headers ? { ...options.headers } : undefined });
		this.url = new URL(this[HANDLE].url() ?? String(options.url));
	}

	get sessionId(): string | undefined {
		return this[HANDLE].sessionId();
	}

	setProtocolVersion(version: string): void {
		this[HANDLE].setProtocolVersion(version);
	}
}
