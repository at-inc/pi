// Node host: global fetch and WebSocket (looked up per call, so tests can replace them), timers, process.env,
// node:fs, child processes (process.ts) and HTTP servers (http-server.ts). Importing the package on Node (the default
// export condition) loads the wasm next to the package with it, or, in a single executable application, the SEA asset
// named `pi_wasm_bg.wasm`.

import { existsSync, readFileSync } from "node:fs";
import { readFile } from "node:fs/promises";
import { createServer } from "node:http";
import { homedir } from "node:os";
import { initCore, type PiHost } from "./core.ts";
import { listenWith } from "./http-server.ts";
import { spawnProcess } from "./process.ts";

/** The host the Node entry installs. Its methods may be replaced at runtime: the core reads them per call. */
export const host: PiHost = {
	fetch: (url, init) => globalThis.fetch(url, init),
	now: () => Date.now(),
	monotonic: () => performance.now(),
	env: (name) => process.env[name],
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
	fileExists: (path) => existsSync(path),
	homeDir: () => homedir(),
	readFile: (path) => readFile(path, "utf-8"),
	webSocket: (url, headers) =>
		typeof globalThis.WebSocket === "function"
			? new globalThis.WebSocket(url, { headers } as unknown as string[])
			: undefined,
	spawnProcess,
	listen: listenWith(createServer),
	runtimeDescription: () =>
		`${process.platform}; ${process.versions.bun ? `bun/${process.versions.bun}` : `node/${process.version}`}; ${process.arch}`,
};

const WASM_FILE = "pi_wasm_bg.wasm";

// Bun has no node:sea, so it is looked up instead of imported.
const sea = process.getBuiltinModule?.("node:sea");

/** The initialized wasm exports. */
export const wasm = initCore(
	host,
	sea?.isSea() ? sea.getAsset(WASM_FILE) : readFileSync(new URL(`../wasm/${WASM_FILE}`, import.meta.url)),
);
