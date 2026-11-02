import { spawnSync } from "node:child_process";
import { mkdtempSync, readFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";
import type { ModelsDevV2Catalog } from "../catalog/sources/models-dev.ts";
import type { Api, Model } from "../src/types.ts";
import { writeUpstreamMock } from "./catalog-generation-utils.ts";

export function generateModelsFixture(catalog: ModelsDevV2Catalog): Record<string, Record<string, Model<Api>>> {
	const root = mkdtempSync(join(tmpdir(), "pi-models-fixture-"));
	try {
		const preloadPath = join(root, "mock-catalog.mjs");
		const outputPath = join(root, "catalog");
		writeUpstreamMock(preloadPath, catalog);
		const result = spawnSync(
			process.execPath,
			[
				"--import",
				pathToFileURL(preloadPath).href,
				"catalog/generate.ts",
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
