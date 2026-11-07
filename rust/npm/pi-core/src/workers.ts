// Workers host: fetch and WebSocket, timers and `process.env` when nodejs_compat provides it; no file system and no
// processes, like the TS packages on Workers. The wasm is imported as a compiled WebAssembly.Module, which wrangler
// and workerd provide for .wasm imports.

import wasmModule from "../wasm/pi_wasm_bg.wasm";
import { initCore, type PiHost } from "./core.ts";

interface ProcessLike {
	env?: Record<string, string | undefined>;
}

/** The host the Workers entry installs. */
export const host: PiHost = {
	fetch: (url, init) => globalThis.fetch(url, init),
	now: () => Date.now(),
	monotonic: () => performance.now(),
	env: (name) => (globalThis as { process?: ProcessLike }).process?.env?.[name],
	sleep: (ms) => new Promise((resolve) => setTimeout(resolve, ms)),
	fileExists: () => false,
	homeDir: () => undefined,
	readFile: (path) =>
		Promise.reject(
			Object.assign(new Error(`ENOENT: no such file or directory, open '${path}'`), {
				errno: -2,
				code: "ENOENT",
				syscall: "open",
				path,
			}),
		),
	webSocket: (url, headers) =>
		typeof globalThis.WebSocket === "function"
			? new globalThis.WebSocket(url, { headers } as unknown as string[])
			: undefined,
};

/** The initialized wasm exports. */
export const wasm = initCore(host, wasmModule);

export * from "./events.ts";
