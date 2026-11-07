// Runs the facade on workerd: bundles test/workers/worker.ts with the "workerd" export condition (so @at-inc/pi-core
// loads its Workers host and imports the wasm as a compiled module), starts it in Miniflare, and streams one response
// from a local Chat Completions server through it.
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

const server = createServer((_request, response) => {
	response.writeHead(200, { "content-type": "text/event-stream" });
	for (const chunk of [
		{ id: "c1", model: "m", choices: [{ index: 0, delta: { content: "Hel" } }] },
		{ id: "c1", model: "m", choices: [{ index: 0, delta: { content: "lo" } }] },
		{
			id: "c1",
			model: "m",
			choices: [{ index: 0, delta: {}, finish_reason: "stop" }],
			usage: { prompt_tokens: 3, completion_tokens: 2 },
		},
	]) {
		response.write(`data: ${JSON.stringify(chunk)}\n\n`);
	}
	response.end("data: [DONE]\n\n");
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
		body: `http://127.0.0.1:${port}/v1`,
	});
	const { events, message } = (await response.json()) as {
		events: string[];
		message: { content: Array<{ text?: string }>; stopReason: string; usage: { input: number; output: number } };
	};
	assert.deepEqual(events, ["start", "text_start", "text_delta", "text_delta", "text_end", "done"]);
	assert.equal(message.content[0]?.text, "Hello");
	assert.equal(message.stopReason, "stop");
	assert.deepEqual([message.usage.input, message.usage.output], [3, 2]);
	console.log("workerd: streamed", JSON.stringify(message.content), message.stopReason);
} finally {
	await miniflare.dispose();
	server.close();
}
