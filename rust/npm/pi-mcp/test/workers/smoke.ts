// Runs the facade on workerd: bundles test/workers/worker.ts with the "workerd" export condition (so @at-inc/pi-core
// loads its Workers host and imports the wasm as a compiled module), starts it in Miniflare, and runs an MCP session
// against a local Streamable HTTP server through it.
//
// Run: node test/workers/smoke.ts (after npm run build:wasm)

import assert from "node:assert/strict";
import { readdirSync } from "node:fs";
import { createServer } from "node:http";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { build } from "esbuild";
import { convertV4MiniflareOptions, Miniflare } from "miniflare";

const here = dirname(fileURLToPath(import.meta.url));

const results: Record<string, unknown> = {
	initialize: {
		protocolVersion: "2025-11-25",
		capabilities: { tools: {} },
		serverInfo: { name: "smoke-server", version: "1.0.0" },
	},
	"tools/list": { tools: [{ name: "echo", inputSchema: { type: "object" } }] },
};

const server = createServer((request, response) => {
	if (request.method !== "POST") {
		response.writeHead(405).end();
		return;
	}
	const chunks: Buffer[] = [];
	request.on("data", (chunk: Buffer) => chunks.push(chunk));
	request.on("end", () => {
		const message = JSON.parse(Buffer.concat(chunks).toString("utf8"));
		if (!("id" in message)) {
			response.writeHead(202).end();
			return;
		}
		const result = results[message.method] ?? { content: [{ type: "text", text: message.params.arguments.text }] };
		response.writeHead(200, { "content-type": "application/json", "mcp-session-id": "s1" });
		response.end(JSON.stringify({ jsonrpc: "2.0", id: message.id, result }));
	});
});
await new Promise<void>((resolve) => server.listen(0, "127.0.0.1", resolve));
const { port } = server.address() as { port: number };

const outdir = join(here, "../../dist/workers-smoke");
await build({
	entryPoints: [join(here, "worker.ts")],
	outdir,
	bundle: true,
	format: "esm",
	platform: "neutral",
	conditions: ["workerd", "source"],
	loader: { ".wasm": "copy" },
	logLevel: "warning",
});

const miniflare = new Miniflare(
	convertV4MiniflareOptions({
		modulesRoot: outdir,
		modules: [
			{ type: "ESModule", path: join(outdir, "worker.js") },
			...readdirSync(outdir)
				.filter((file) => file.endsWith(".wasm"))
				.map((file) => ({ type: "CompiledWasm" as const, path: join(outdir, file) })),
		],
		compatibilityDate: "2026-10-01",
	}),
);
try {
	const response = await miniflare.dispatchFetch("http://worker/", {
		method: "POST",
		body: `http://127.0.0.1:${port}/mcp`,
	});
	const { tools, result, stdio } = (await response.json()) as {
		tools: unknown[];
		result: unknown;
		stdio: { message: string; code?: string } | undefined;
	};
	assert.deepEqual(tools, [{ name: "echo", inputSchema: { type: "object" } }]);
	assert.deepEqual(result, { content: [{ type: "text", text: "hello" }] });
	assert.equal(stdio?.code, "ERR_METHOD_NOT_IMPLEMENTED");
	console.log("workerd: MCP session", JSON.stringify(result), "stdio:", stdio?.message);
} finally {
	await miniflare.dispose();
	server.close();
}
