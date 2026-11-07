// PiHost.listen over a node:http module, shared by the Node and Workers hosts. The Node host passes its static import;
// the Workers host passes nodejs_compat's module, so its entry does not need nodejs_compat for anything else.

import type * as http from "node:http";
import type { PiHost } from "./core.ts";

/** `PiHost.listen` as `http.createServer(handler).listen(port, host)`, the way the TS OAuth callback servers call it. */
export const listenWith =
	(createServer: typeof http.createServer): PiHost["listen"] =>
	(host, port, handler, onError) =>
		new Promise((resolve, reject) => {
			const server = createServer((request, response) => {
				void handler({ method: request.method ?? "", url: request.url ?? "/" }).then(
					({ status, headers, body }) => {
						response.writeHead(status, headers);
						response.end(body);
					},
					() => {
						response.writeHead(500);
						response.end();
					},
				);
			});
			server.once("error", reject);
			server.listen(port, host, () => {
				server.off("error", reject);
				const address = server.address();
				if (!address || typeof address === "string") {
					server.close();
					reject(new Error("OAuth callback server did not bind to TCP"));
					return;
				}
				server.on("error", onError);
				resolve({
					port: address.port,
					close: () => server.close(),
					closeAllConnections: () => server.closeAllConnections(),
				});
			});
		});
