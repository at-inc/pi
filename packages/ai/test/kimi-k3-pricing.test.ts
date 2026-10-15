import { mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { afterEach, describe, expect, it, vi } from "vitest";
import { applyForkModelMetadata } from "../scripts/fork-model-data.ts";
import { hydrateModelCatalog } from "../scripts/hydrate-model-catalog.ts";
import { validateGeneratedModelData } from "../scripts/model-data.ts";
import type { Model } from "../src/types.ts";
import { generateModelsFixture } from "./generate-models-fixture.ts";

const targets = [
	["moonshotai", "kimi-k3"],
	["moonshotai-cn", "kimi-k3"],
	["kimi-coding", "k3"],
] as const;

afterEach(() => vi.unstubAllGlobals());

describe("Kimi K3 cache-write pricing", () => {
	it("uses the approved rates when generating from stale upstream metadata", () => {
		const stale = {
			tool_call: true,
			reasoning: true,
			cost: { input: 3, output: 15, cache_read: 0.3, cache_write: 0 },
		};
		const catalog = generateModelsFixture({
			moonshotai: { models: { "kimi-k3": stale } },
			"moonshotai-cn": { models: { "kimi-k3": stale } },
			"kimi-code-plan-global": { models: { k3: stale } },
		});
		for (const [provider, id] of targets) {
			expect(catalog[provider][id].cost).toEqual({ input: 3, output: 15, cacheRead: 0.3, cacheWrite: 3 });
		}
	});

	it("corrects immutable catalog hydration without changing other rates or the catalog bytes", () => {
		const fetch = vi.fn(() => {
			throw new Error("Unexpected network request");
		});
		vi.stubGlobal("fetch", fetch);
		const root = mkdtempSync(join(tmpdir(), "pi-kimi-k3-pricing-"));
		try {
			mkdirSync(join(root, "src/providers"), { recursive: true });
			const catalog: Record<string, Model<"openai-completions">[]> = {};
			for (const [provider, id] of targets) {
				writeFileSync(join(root, `src/providers/${provider}.models.ts`), "");
				catalog[provider] = [
					{
						type: "chat",
						id,
						name: id,
						provider,
						api: "openai-completions",
						baseUrl: "https://example.invalid/v1",
						reasoning: true,
						input: ["text"],
						contextWindow: 1000000,
						maxTokens: 131072,
						cost: { input: 3, output: 15, cacheRead: 0.3, cacheWrite: 0 },
					},
				];
			}
			writeFileSync(
				join(root, "src/models.generated.ts"),
				targets
					.map(([provider]) => {
						const prefix = provider.toUpperCase().replace(/-/g, "_");
						return `import { ${prefix}_CLASSIFIER_MODELS, ${prefix}_IMAGE_MODELS, ${prefix}_MODELS } from "./providers/${provider}.models.ts";`;
					})
					.join("\n"),
			);
			const path = join(root, "models.all.json");
			const bytes = JSON.stringify(catalog);
			writeFileSync(path, bytes);
			hydrateModelCatalog(root, path);
			validateGeneratedModelData(root);
			for (const [provider, id] of targets) {
				const data = JSON.parse(readFileSync(join(root, `src/providers/data/${provider}.json`), "utf8")) as Record<
					string,
					Record<string, Model<"openai-completions">>
				>;
				expect(data["openai-completions"][`chat:${id}`]).toEqual({
					...catalog[provider][0],
					cost: { ...catalog[provider][0].cost, cacheWrite: 3 },
				});
			}
			expect(readFileSync(path, "utf8")).toBe(bytes);
			expect(fetch).not.toHaveBeenCalled();
		} finally {
			rmSync(root, { recursive: true, force: true });
		}
	});

	it("does not change pricing on other providers, model IDs, or APIs", () => {
		const models = [
			{ provider: "together", id: "moonshotai/Kimi-K3", api: "openai-completions" },
			{ provider: "kimi-coding", id: "kimi-for-coding-highspeed", api: "openai-completions" },
			{ provider: "moonshotai", id: "kimi-k3", api: "anthropic-messages" },
		].map((model) => ({ ...model, cost: { input: 1, output: 2, cacheRead: 0.1, cacheWrite: 0 } }));
		const before = structuredClone(models);
		for (const model of models) applyForkModelMetadata(model);
		expect(models).toEqual(before);
	});
});
