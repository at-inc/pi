// session-resources: the registry of per-session cleanups, shared with the Rust core's own cleanups.

import type * as ts from "@at-inc/pi-ai/session-resources";
import { wasm } from "@at-inc/pi-core";

export type * from "@at-inc/pi-ai/session-resources";

export const registerSessionResourceCleanup: typeof ts.registerSessionResourceCleanup = (cleanup) =>
	wasm.aiRegisterSessionResourceCleanup(cleanup) as () => void;

export const cleanupSessionResources: typeof ts.cleanupSessionResources = (sessionId) => {
	try {
		wasm.aiCleanupSessionResources(sessionId);
	} catch (error) {
		// The core reports the cleanups' errors as `errors` of an Error named AggregateError.
		if (error instanceof Error && error.name === "AggregateError" && !(error instanceof AggregateError)) {
			throw new AggregateError((error as { errors?: unknown[] }).errors ?? [], error.message);
		}
		throw error;
	}
};
