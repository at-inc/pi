// utils/sleep: JS, as in the TS: a timer that rejects with the caller's signal's own reason.

import type * as ts from "@at-inc/pi-ai/utils/sleep";

export type * from "@at-inc/pi-ai/utils/sleep";

export const sleep: typeof ts.sleep = (ms, signal) =>
	new Promise((resolve, reject) => {
		signal.throwIfAborted();
		const onAbort = () => {
			clearTimeout(timeout);
			reject(signal.reason);
		};
		const timeout = setTimeout(() => {
			signal.removeEventListener("abort", onAbort);
			resolve();
		}, ms);
		signal.addEventListener("abort", onAbort, { once: true });
	});
