// The base of the facade transports: a Rust transport handle behind the McpTransport methods. McpClient.connect
// passes the handle back to Rust, so messages between the Rust client and a Rust transport never cross into JS.

import type { McpTransportHandle } from "@at-inc/pi-core/wasm";
import type { JsonRpcMessage, McpTransport, McpTransportSendOptions } from "@earendil-works/pi-mcp";
import { errorListener, settle } from "./errors.ts";

/** The key of the Rust transport of a facade transport. */
export const HANDLE = Symbol("pi-core McpTransportHandle");

/** A transport whose implementation is a Rust transport. */
export abstract class NativeTransport implements McpTransport {
	readonly [HANDLE]: McpTransportHandle;

	constructor(handle: McpTransportHandle) {
		this[HANDLE] = handle;
	}

	start(): Promise<void> {
		return settle(this[HANDLE].start());
	}

	send(message: JsonRpcMessage, options: McpTransportSendOptions = {}): Promise<void> {
		return settle(this[HANDLE].send(message, options));
	}

	close(): Promise<void> {
		return settle(this[HANDLE].close());
	}

	onMessage(listener: (message: JsonRpcMessage) => void): () => void {
		return this[HANDLE].onMessage(listener);
	}

	onError(listener: (error: Error) => void): () => void {
		return this[HANDLE].onError(errorListener(listener));
	}

	onClose(listener: () => void): () => void {
		return this[HANDLE].onClose(listener);
	}
}

/** The Rust transport of a facade transport, `undefined` for a transport written in JS. */
export function transportHandle(transport: McpTransport): McpTransportHandle | undefined {
	return transport instanceof NativeTransport ? transport[HANDLE] : undefined;
}
