// Workers host: fetch and WebSocket, timers and `process.env` when nodejs_compat provides it; no file system and no
// processes, like the TS packages on Workers. HTTP servers are nodejs_compat's node:http, which listens on workerd
// without receiving outside requests, as the TS callback servers do there; without nodejs_compat `listen` rejects.
// The wasm is imported as a compiled WebAssembly.Module, which wrangler and workerd provide for .wasm imports.

import type * as http from "node:http";
import wasmModule from "../wasm/pi_wasm_bg.wasm";
import { initCore, type PiHost } from "./core.ts";
import { listenWith } from "./http-server.ts";

interface ProcessLike {
	env?: Record<string, string | undefined>;
	getBuiltinModule?: (id: "node:http") => typeof http | undefined;
	versions?: { node?: string; bun?: string };
	version?: string;
	platform?: string;
	arch?: string;
}

const nodeHttp = (globalThis as { process?: ProcessLike }).process?.getBuiltinModule?.("node:http");

/** The host the Workers entry installs. */
export const host: PiHost = {
	fetch: (url, init) => globalThis.fetch(url, init),
	now: () => Date.now(),
	monotonic: () => performance.now(),
	env: (name) => (globalThis as { process?: ProcessLike }).process?.env?.[name],
	sleep: (ms, signal) =>
		new Promise((resolve) => {
			const timer = setTimeout(resolve, ms);
			signal?.addEventListener(
				"abort",
				() => {
					clearTimeout(timer);
					resolve();
				},
				{ once: true },
			);
		}),
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
	spawnProcess: () => undefined,
	listen: nodeHttp
		? listenWith(nodeHttp.createServer)
		: () => Promise.reject(new Error("HTTP servers are not supported on this host")),
	runtimeDescription: () => {
		const process = (globalThis as { process?: ProcessLike }).process;
		const versions = process?.versions;
		const runtime = versions?.bun ? `bun/${versions.bun}` : versions?.node ? `node/${process?.version}` : undefined;
		return runtime && `${process?.platform}; ${runtime}; ${process?.arch}`;
	},
};

/** The initialized wasm exports. */
export const wasm = initCore(host, wasmModule);

export * from "./events.ts";
