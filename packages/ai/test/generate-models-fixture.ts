import { spawnSync } from "node:child_process";
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";
import type { Api, Model } from "../src/types.ts";

export function generateModelsFixture(
	catalog: Record<string, { models: Record<string, unknown> }>,
): Record<string, Record<string, Model<Api>>> {
	const root = mkdtempSync(join(tmpdir(), "pi-models-fixture-"));
	try {
		const preloadPath = join(root, "mock-catalog.mjs");
		const outputPath = join(root, "catalog");
		writeFileSync(
			preloadPath,
			`const catalog = ${JSON.stringify(catalog)};
globalThis.fetch = async (input) => {
  const url = String(input);
  if (url === "https://models.dev/api.json") return Response.json(catalog);
  if (url === "https://models.dev/models.json?type=decision") return Response.json({ "typesafe/jev-latest": { name: "Jev", type: "decision", limit: { context: 64000 } } });
  if (url.startsWith("https://openrouter.ai/api/v1/models") || url === "https://ai-gateway.vercel.sh/v1/models") return Response.json({ data: [] });
  if (url === "https://radius.pi.dev/v1/config") return Response.json({ baseUrl: "https://radius.pi.dev", models: [{ id: "test", name: "Test", reasoning: false, input: ["text"], cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0 }, contextWindow: 4096, maxTokens: 4096 }] });
  throw new Error(\`Unexpected fetch: \${url}\`);
};
`,
		);
		const result = spawnSync(
			process.execPath,
			[
				"--import",
				pathToFileURL(preloadPath).href,
				"scripts/generate-models.ts",
				"--json-only",
				"--json-output",
				outputPath,
			],
			{ cwd: fileURLToPath(new URL("..", import.meta.url)), encoding: "utf8", timeout: 10_000 },
		);
		if (result.status !== 0 || result.stderr) {
			throw new Error(`Model fixture generation failed: ${result.stdout}\n${result.stderr}`, {
				cause: result.error,
			});
		}
		return JSON.parse(readFileSync(join(outputPath, "models.json"), "utf8"));
	} finally {
		rmSync(root, { force: true, recursive: true });
	}
}
