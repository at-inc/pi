// HTTP servers of the Node host: `node:http` servers that pass the request line and headers to the core's handler
// and send its answer, as the OAuth callback servers of the TS packages do.

import type { IncomingHttpHeaders } from "node:http";
import { createServer } from "node:http";
import type { PiHttpServer, PiServerRequest, PiServerResponse } from "../wasm/pi_wasm.js";

/** `PiHost.listenHttp` on Node. */
export async function listenHttp(
	host: string,
	port: number,
	handler: (request: PiServerRequest) => Promise<PiServerResponse>,
): Promise<PiHttpServer> {
	const server = createServer((request, response) => {
		const answer = handler({
			method: request.method ?? "GET",
			url: request.url ?? "/",
			headers: headers(request.headers),
		});
		answer.then(
			(reply) => response.writeHead(reply.status, reply.headers).end(reply.body),
			(error: unknown) => {
				response.statusCode = 500;
				response.end(String(error));
			},
		);
	});
	await new Promise<void>((resolve, reject) => {
		server.once("error", reject);
		server.listen(port, host, () => {
			server.off("error", reject);
			resolve();
		});
	});
	const address = server.address();
	if (!address || typeof address === "string") throw new Error("HTTP server did not bind to TCP");
	return {
		port: address.port,
		close: () => new Promise<void>((resolve, reject) => server.close((error) => (error ? reject(error) : resolve()))),
	};
}

function headers(incoming: IncomingHttpHeaders): Record<string, string> {
	const out: Record<string, string> = {};
	for (const [name, value] of Object.entries(incoming)) {
		if (value !== undefined) out[name] = Array.isArray(value) ? value.join(", ") : value;
	}
	return out;
}
