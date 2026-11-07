// Node host: global fetch and WebSocket (looked up per call, so tests can replace them), timers, process.env and
// node:fs. Importing the package on Node (the default export condition) loads the wasm next to the package with it.

import { existsSync, readFileSync } from "node:fs";
import { readFile } from "node:fs/promises";
import { homedir } from "node:os";
import { initCore, type PiHost } from "./core.ts";

/** The host the Node entry installs. Its methods may be replaced at runtime: the core reads them per call. */
export const host: PiHost = {
	fetch: (url, init) => globalThis.fetch(url, init),
	now: () => Date.now(),
	monotonic: () => performance.now(),
	env: (name) => process.env[name],
	sleep: (ms) => new Promise((resolve) => setTimeout(resolve, ms)),
	fileExists: (path) => existsSync(path),
	homeDir: () => homedir(),
	readFile: (path) => readFile(path, "utf-8"),
	webSocket: (url, headers) =>
		typeof globalThis.WebSocket === "function"
			? new globalThis.WebSocket(url, { headers } as unknown as string[])
			: undefined,
};

/** The initialized wasm exports. */
export const wasm = initCore(host, readFileSync(new URL("../wasm/pi_wasm_bg.wasm", import.meta.url)));
