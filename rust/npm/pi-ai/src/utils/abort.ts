// utils/abort: JS, as in the TS. Both functions work on the caller's AbortSignal objects (`operationSignal(signal)` is
// `signal` itself), which a crossing into the core would replace.

import type * as ts from "@at-inc/pi-ai/utils/abort";

export type * from "@at-inc/pi-ai/utils/abort";

function abortReason(signal: AbortSignal): unknown {
	if (signal.reason !== undefined) return signal.reason;
	const error = new Error("The operation was aborted");
	error.name = "AbortError";
	return error;
}

export const operationSignal: typeof ts.operationSignal = (signal) => signal ?? new AbortController().signal;

export const raceWithAbortSignal: typeof ts.raceWithAbortSignal = (operation, signal) => {
	if (signal.aborted) {
		void operation.catch(() => {});
		return Promise.reject(abortReason(signal));
	}
	return new Promise((resolve, reject) => {
		let settled = false;
		const cleanup = () => signal.removeEventListener("abort", onAbort);
		const onAbort = () => {
			if (settled) return;
			settled = true;
			cleanup();
			reject(abortReason(signal));
		};
		signal.addEventListener("abort", onAbort, { once: true });
		void operation.then(
			(value) => {
				if (settled) return;
				settled = true;
				cleanup();
				resolve(value);
			},
			(error: unknown) => {
				if (settled) return;
				settled = true;
				cleanup();
				reject(error);
			},
		);
		if (signal.aborted) onAbort();
	});
};
