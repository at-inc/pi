// transports/stdio: StdioTransport over the Rust transport, which spawns the server through the host's
// spawnProcess (Node: cross-spawn in its own process group; Workers: unsupported, as in the TS).

import { wasm } from "@at-inc/pi-core";
import type { StdioTransportOptions } from "@earendil-works/pi-mcp";
import { attempt } from "../internal/errors.ts";
import { HANDLE, NativeTransport } from "../internal/native-transport.ts";

export type { StdioTransportOptions } from "@earendil-works/pi-mcp";

/** `StdioTransport`: newline-delimited JSON-RPC over a child process's stdin and stdout. */
export class StdioTransport extends NativeTransport {
	readonly options: Readonly<StdioTransportOptions>;

	constructor(options: StdioTransportOptions) {
		super(attempt(() => wasm.mcpCreateStdioTransport(options)));
		this.options = Object.freeze({ ...options, args: options.args ? [...options.args] : undefined });
	}

	get pid(): number | undefined {
		return this[HANDLE].pid();
	}

	get stderr(): string {
		return this[HANDLE].stderr();
	}
}
