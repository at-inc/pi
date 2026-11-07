// Runs the Workers host's HTTP listener on workerd: bundles test/workers/listen-worker.ts like smoke.ts does and starts
// it twice. With a current compatibility date workerd provides node:http, so the callback server binds a loopback
// port; with a date before node:http existed there, startOAuthCallbackServer rejects as the host cannot listen.
//
// Run: node test/workers/listen.ts (after npm run build:wasm)

import assert from "node:assert/strict";
import { readdirSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { build } from "esbuild";
import { convertV4MiniflareOptions, Miniflare } from "miniflare";

const here = dirname(fileURLToPath(import.meta.url));
const outdir = join(here, "../../dist/workers-listen");
await build({
	entryPoints: [join(here, "listen-worker.ts")],
	outdir,
	bundle: true,
	format: "esm",
	platform: "neutral",
	conditions: ["workerd", "source"],
	loader: { ".wasm": "copy" },
	logLevel: "warning",
});

async function run(compatibilityDate: string): Promise<{ redirectUri?: string; error?: string }> {
	const miniflare = new Miniflare(
		convertV4MiniflareOptions({
			modulesRoot: outdir,
			modules: [
				{ type: "ESModule", path: join(outdir, "listen-worker.js") },
				...readdirSync(outdir)
					.filter((file) => file.endsWith(".wasm"))
					.map((file) => ({ type: "CompiledWasm" as const, path: join(outdir, file) })),
			],
			compatibilityDate,
		}),
	);
	try {
		return (await (await miniflare.dispatchFetch("http://worker/")).json()) as {
			redirectUri?: string;
			error?: string;
		};
	} finally {
		await miniflare.dispose();
	}
}

const current = await run("2026-10-01");
assert.match(current.redirectUri ?? String(current.error), /^http:\/\/127\.0\.0\.1:\d+\/callback$/);
console.log("workerd: listening at", current.redirectUri);

const old = await run("2024-01-01");
assert.equal(old.error, "HTTP servers are not supported on this host");
console.log("workerd without node:http:", old.error);
