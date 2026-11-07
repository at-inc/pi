// The facade of stream-fn.ts: the default stream function, kept in JS for `Agent` (which passes its stream function
// with each run) and in the core for the loops run without one.

import type * as ts from "@at-inc/pi-agent-core";
import type { StreamFn } from "@at-inc/pi-agent-core";
import { wasm } from "@at-inc/pi-core";

let defaultStreamFn: StreamFn | undefined;

/**
 * Configure the fallback used by Agent and low-level loops when callers omit streamFn.
 *
 * Hosts that provide a default model runtime can install its stream function here
 * without making pi-agent-core depend on a provider catalog or compatibility layer.
 */
export const setDefaultStreamFn: typeof ts.setDefaultStreamFn = (streamFn) => {
	defaultStreamFn = streamFn;
	wasm.agentSetDefaultStreamFn(streamFn);
};

/** The configured fallback stream function. Throws when none is configured. */
export function getDefaultStreamFn(): StreamFn {
	if (!defaultStreamFn) {
		throw new Error("No default stream function configured. Pass streamFn explicitly or call setDefaultStreamFn().");
	}
	return defaultStreamFn;
}
