// A Worker that starts the OAuth callback server through the facade, for test/workers/listen.ts. The response is the
// server's redirect URI, or the error startOAuthCallbackServer rejects with.

import { startOAuthCallbackServer } from "../../src/auth/oauth/callback-server.ts";

export default {
	async fetch(): Promise<Response> {
		try {
			const server = await startOAuthCallbackServer({
				providerName: "Test",
				host: "127.0.0.1",
				port: 0,
				path: "/callback",
				complete: async (code) => code,
			});
			server.close();
			return Response.json({ redirectUri: server.redirectUri });
		} catch (error) {
			return Response.json({ error: error instanceof Error ? error.message : String(error) });
		}
	},
};
