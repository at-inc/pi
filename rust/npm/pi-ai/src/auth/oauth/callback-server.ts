// auth/oauth/callback-server: the loopback OAuth redirect server, listening through the host (node:http).

import type * as ts from "@at-inc/pi-ai/auth/oauth/callback-server";
import { wasm } from "@at-inc/pi-core";
import type { OAuthCallbackServerHandle } from "@at-inc/pi-core/wasm";
import { notBridged } from "../../internal/not-bridged.ts";

export type * from "@at-inc/pi-ai/auth/oauth/callback-server";

const HANDLE = Symbol("pi-core OAuthCallbackServerHandle");

/** A server of startOAuthCallbackServer: the TS object over a Rust server; `wait()` returns one promise. */
interface RustOAuthCallbackServer<T> extends ts.OAuthCallbackServer<T> {
	readonly [HANDLE]: OAuthCallbackServerHandle;
}

export const startOAuthCallbackServer: typeof ts.startOAuthCallbackServer = async <T>(
	options: ts.OAuthCallbackServerOptions<T>,
) => {
	const handle: OAuthCallbackServerHandle = await wasm.aiStartOAuthCallbackServer(options);
	const waitPromise: Promise<T | undefined> = handle.wait();
	// A cancelled or closed wait may never be observed.
	waitPromise.catch(() => undefined);
	const server: RustOAuthCallbackServer<T> = {
		redirectUri: handle.redirectUri,
		wait: () => waitPromise,
		cancel: () => handle.cancel(),
		close: () => handle.close(),
		[HANDLE]: handle,
	};
	return server;
};

export const waitForCallbackOrManualInput: typeof ts.waitForCallbackOrManualInput = async (
	interaction,
	callback,
	prompt,
) => {
	const handle = (callback as Partial<RustOAuthCallbackServer<unknown>> | undefined)?.[HANDLE];
	if (callback && !handle)
		notBridged("waitForCallbackOrManualInput with a server not made by startOAuthCallbackServer");
	return wasm.aiWaitForCallbackOrManualInput(interaction, handle?.share(), prompt);
};
