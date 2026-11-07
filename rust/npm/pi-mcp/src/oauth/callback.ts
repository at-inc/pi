// oauth/callback: OAuthCallbackServer over the Rust server, which listens through the host's listenHttp (Node:
// node:http; Workers: unsupported, as in the TS).

import { wasm } from "@at-inc/pi-core";
import type { OAuthCallbackServerHandle } from "@at-inc/pi-core/wasm";
import type { OAuthCallback, OAuthCallbackServerOptions } from "@earendil-works/pi-mcp/oauth";
import { attempt, settle } from "../internal/errors.ts";

export type { OAuthCallback, OAuthCallbackPage, OAuthCallbackServerOptions } from "@earendil-works/pi-mcp/oauth";

/** `OAuthCallbackServer`: a loopback HTTP server that receives OAuth authorization responses. */
export class OAuthCallbackServer {
	readonly redirectUrl: string;
	private handle: OAuthCallbackServerHandle;

	private constructor(handle: OAuthCallbackServerHandle) {
		this.handle = handle;
		this.redirectUrl = handle.redirectUrl();
	}

	static async listen(options: OAuthCallbackServerOptions = {}): Promise<OAuthCallbackServer> {
		return new OAuthCallbackServer(await settle(wasm.mcpListenOAuthCallbackServer(options)));
	}

	/**
	 * Wait for the authorization response with `state`. With `path`, a response on another path fails, so
	 * a server-specific redirect URI can tell authorization servers apart (RFC 9700 section 4.4.2.2).
	 */
	waitForCallback(state: string, path?: string): Promise<OAuthCallback> {
		return settle(attempt(() => this.handle.waitForCallback(state, path)));
	}

	close(): Promise<void> {
		return settle(this.handle.close());
	}
}
