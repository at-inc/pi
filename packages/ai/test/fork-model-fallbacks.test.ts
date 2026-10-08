import { mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { afterEach, describe, expect, it, vi } from "vitest";
import { getForkChatModelFallbacks } from "../scripts/fork-model-fallbacks.ts";
import { hydrateModelCatalog } from "../scripts/hydrate-model-catalog.ts";
import { MODEL_DATA_MANIFEST_FILE, validateGeneratedModelData } from "../scripts/model-data.ts";
import type { AnyModel } from "../src/types.ts";

const roots: string[] = [];

afterEach(() => {
	vi.unstubAllGlobals();
	for (const root of roots.splice(0)) rmSync(root, { recursive: true, force: true });
});

function fixture(models: readonly AnyModel[]): { root: string; catalogPath: string; dataPath: string } {
	const root = mkdtempSync(join(tmpdir(), "pi-fork-model-fallbacks-"));
	roots.push(root);
	mkdirSync(join(root, "src/providers"), { recursive: true });
	writeFileSync(join(root, "src/providers/anthropic.models.ts"), "");
	writeFileSync(
		join(root, "src/models.generated.ts"),
		'import { ANTHROPIC_CLASSIFIER_MODELS, ANTHROPIC_IMAGE_MODELS, ANTHROPIC_MODELS } from "./providers/anthropic.models.ts";',
	);
	const catalogPath = join(root, "models.all.json");
	writeFileSync(catalogPath, JSON.stringify({ anthropic: models }));
	return { root, catalogPath, dataPath: join(root, "src/providers/data/anthropic.json") };
}

describe("fork chat model fallbacks", () => {
	it("matches the complete generated Haiku 5.5 metadata without adding Fast support", () => {
		const data = JSON.parse(
			readFileSync(new URL("../src/providers/data/anthropic.json", import.meta.url), "utf8"),
		) as Record<string, Record<string, unknown>>;
		const [fallback] = getForkChatModelFallbacks("anthropic");
		expect(fallback).toEqual(data["anthropic-messages"]["chat:claude-haiku-5-5"]);
		expect(fallback.supportsFastMode).toBeUndefined();
		expect(getForkChatModelFallbacks("openai")).toEqual([]);
	});

	it("returns isolated metadata for each use", () => {
		const [model] = getForkChatModelFallbacks("anthropic");
		const before = structuredClone(model);
		model.cost.tiers![0].input = 99;
		model.thinkingLevelMap!.high = null;
		model.compat!.supportsStrictTools = false;
		model.inputLimits!.images!.resize!.maxWidth = 1;
		expect(getForkChatModelFallbacks("anthropic")).toEqual([before]);
	});

	it("hydrates missing Haiku metadata offline and deterministically without changing existing entries", () => {
		const fetch = vi.fn(() => {
			throw new Error("Unexpected network request");
		});
		vi.stubGlobal("fetch", fetch);
		const [fallback] = getForkChatModelFallbacks("anthropic");
		const existing = { ...fallback, id: "existing-chat", name: "Existing model", custom: true };
		const { root, catalogPath, dataPath } = fixture([existing]);
		hydrateModelCatalog(root, catalogPath);
		validateGeneratedModelData(root);
		const first = readFileSync(dataPath, "utf8");
		const manifestPath = join(root, "src/providers/data", MODEL_DATA_MANIFEST_FILE);
		const manifest = readFileSync(manifestPath, "utf8");
		const data = JSON.parse(first) as Record<string, Record<string, unknown>>;
		expect(data["anthropic-messages"]).toEqual({
			"chat:existing-chat": existing,
			"chat:claude-haiku-5-5": fallback,
		});
		hydrateModelCatalog(root, catalogPath);
		expect(readFileSync(dataPath, "utf8")).toBe(first);
		expect(readFileSync(manifestPath, "utf8")).toBe(manifest);
		expect(fetch).not.toHaveBeenCalled();
	});

	it("does not replace a supplied Haiku record or its provider-specific metadata", () => {
		const [fallback] = getForkChatModelFallbacks("anthropic");
		const supplied = {
			...fallback,
			name: "Updated upstream Haiku",
			contextWindow: 2000000,
			cost: { ...fallback.cost, input: 0.3 },
			thinkingLevelMap: { high: "custom" },
			compat: { supportsStrictTools: false },
			inputLimits: { images: { maxPerRequest: 20 } },
			custom: { preserve: true },
		};
		const { root, catalogPath, dataPath } = fixture([supplied]);
		hydrateModelCatalog(root, catalogPath);
		validateGeneratedModelData(root);
		const data = JSON.parse(readFileSync(dataPath, "utf8")) as Record<string, Record<string, unknown>>;
		expect(Object.values(data["anthropic-messages"])).toEqual([supplied]);
	});
});
