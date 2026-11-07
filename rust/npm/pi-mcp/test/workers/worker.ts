// A Worker that runs an MCP session through the facade, for test/workers/smoke.ts. The request body is the MCP
// server's URL; the response holds the listed tools, one tool result, and the error of a stdio transport, which
// Workers do not support.

import { McpClient, StdioTransport, StreamableHttpTransport } from "../../src/index.ts";

export default {
	async fetch(request: Request): Promise<Response> {
		const client = new McpClient({ name: "workers-smoke", version: "1.0.0" });
		await client.connect(new StreamableHttpTransport({ url: await request.text() }));
		const tools = await client.listTools();
		const result = await client.callTool("echo", { text: "hello" });
		await client.close();
		const stdio = await new McpClient({ name: "workers-smoke", version: "1.0.0" })
			.connect(new StdioTransport({ command: "node" }))
			.then(
				() => undefined,
				(error: Error & { code?: string }) => ({ message: error.message, code: error.code }),
			);
		return Response.json({ tools, result, stdio });
	},
};
