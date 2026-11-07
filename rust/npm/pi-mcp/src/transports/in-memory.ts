// transports/in-memory: InMemoryTransport over the Rust in-memory transport.

import { wasm } from "@at-inc/pi-core";
import { attempt } from "../internal/errors.ts";
import { HANDLE, NativeTransport } from "../internal/native-transport.ts";

/** `InMemoryTransport`: delivers each message to its peer in a microtask, as a structured clone. */
export class InMemoryTransport extends NativeTransport {
	constructor() {
		super(wasm.mcpCreateInMemoryTransport());
	}

	connectPeer(peer: InMemoryTransport): void {
		attempt(() => this[HANDLE].connectPeer(peer[HANDLE]));
	}

	/** Exposed so tests can simulate transport-level failures. */
	emitError(error: unknown): void {
		this[HANDLE].emitError(error);
	}
}

/** `createInMemoryTransportPair()`: two connected in-memory transports. */
export function createInMemoryTransportPair(): { client: InMemoryTransport; server: InMemoryTransport } {
	const client = new InMemoryTransport();
	const server = new InMemoryTransport();
	client.connectPeer(server);
	server.connectPeer(client);
	return { client, server };
}
