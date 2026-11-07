// Instantiates the wasm module and installs a host. The runtime entries (node.ts, workers.ts) call it with their host;
// other runtimes import "@at-inc/pi-core/core" and pass their own.

import type { PiHost } from "../wasm/pi_wasm.js";
import * as bindings from "../wasm/pi_wasm.js";

export type { PiHost };

/** The wasm exports, typed by the generated `pi_wasm.d.ts`. */
export type Wasm = typeof bindings;

/**
 * Instantiates the wasm module synchronously from its bytes or a compiled `WebAssembly.Module` and installs `host`
 * as the core's platform. Call once per isolate.
 */
export function initCore(host: PiHost, module: ArrayBuffer | ArrayBufferView | WebAssembly.Module): Wasm {
	bindings.initSync({ module });
	bindings.init(host);
	return bindings;
}
