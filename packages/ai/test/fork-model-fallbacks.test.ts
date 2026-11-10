import { mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { afterEach, describe, expect, it, vi } from "vitest";
import { getForkChatModelFallbacks } from "../scripts/fork-model-fallbacks.ts";
import { hydrateModelCatalog } from "../scripts/hydrate-model-catalog.ts";
import { MODEL_DATA_MANIFEST_FILE, validateGeneratedModelData } from "../scripts/model-data.ts";
import { stream } from "../src/api/anthropic-messages.ts";
import { getSupportedThinkingLevels } from "../src/models.ts";
import type { AnyModel } from "../src/types.ts";
import { normalizeContext } from "../src/utils/transcript.ts";

const roots: string[] = [];

afterEach(() => {
	vi.unstubAllGlobals();
	for (const root of roots.splice(0)) rmSync(root, { recursive: true, force: true });
});

function fixture(
	models: readonly AnyModel[],
	provider = "anthropic",
): { root: string; catalogPath: string; dataPath: string } {
	const root = mkdtempSync(join(tmpdir(), "pi-fork-model-fallbacks-"));
	roots.push(root);
	mkdirSync(join(root, "src/providers"), { recursive: true });
	writeFileSync(join(root, `src/providers/${provider}.models.ts`), "");
	const prefix = provider.toUpperCase();
	writeFileSync(
		join(root, "src/models.generated.ts"),
		`import { ${prefix}_CLASSIFIER_MODELS, ${prefix}_IMAGE_MODELS, ${prefix}_MODELS } from "./providers/${provider}.models.ts";`,
	);
	const catalogPath = join(root, "models.all.json");
	writeFileSync(catalogPath, JSON.stringify({ [provider]: models }));
	return { root, catalogPath, dataPath: join(root, `src/providers/data/${provider}.json`) };
}

describe("fork chat model fallbacks", () => {
	// #10753
	it("disables thinking on the Haiku fallback without sending effort settings", async () => {
		const [model] = getForkChatModelFallbacks("anthropic");
		expect(getSupportedThinkingLevels(model)).toEqual(["off", "low", "medium", "high", "xhigh", "max"]);
		const fetch = vi.fn(() => {
			throw new Error("Unexpected provider request");
		});
		let payload: unknown;
		const result = await stream(
			model,
			normalizeContext({ messages: [{ role: "user", content: "Hello", timestamp: 0 }] }),
			{
				apiKey: "test-key",
				cacheRetention: "none",
				thinkingEnabled: false,
				fetch,
				onPayload(value) {
					payload = value;
					throw new Error("Payload captured before network request");
				},
			},
		).result();
		expect(payload).toMatchObject({
			thinking: { type: "disabled" },
			messages: [{ role: "user", content: "Hello" }],
		});
		expect(payload).not.toHaveProperty("output_config");
		expect(result.providerThinkingLevel).toBeUndefined();
		expect(result.errorMessage).toContain("Payload captured before network request");
		expect(fetch).not.toHaveBeenCalled();
	});

	it("matches the complete generated Haiku 5.5 metadata without adding Fast support", () => {
		const data = JSON.parse(
			readFileSync(new URL("../src/providers/data/anthropic.json", import.meta.url), "utf8"),
		) as Record<string, Record<string, unknown>>;
		const [fallback] = getForkChatModelFallbacks("anthropic");
		expect(fallback).toEqual(data["anthropic-messages"]["chat:claude-haiku-5-5"]);
		expect(fallback.supportsFastMode).toBe(false);
		expect(getForkChatModelFallbacks("openai")).toEqual([]);
	});

	it("returns isolated metadata for each use", () => {
		const models = getForkChatModelFallbacks("anthropic");
		const before = structuredClone(models);
		const [model] = models;
		model.cost.tiers![0].input = 99;
		model.thinkingLevelMap!.high = null;
		model.compat!.supportsStrictTools = false;
		model.inputLimits!.images!.resize!.maxWidth = 1;
		models[1].cost.input = 99;
		expect(getForkChatModelFallbacks("anthropic")).toEqual(before);
	});

	it("hydrates missing Haiku metadata offline and deterministically without changing existing entries", () => {
		const fetch = vi.fn(() => {
			throw new Error("Unexpected network request");
		});
		vi.stubGlobal("fetch", fetch);
		const [fallback, fable] = getForkChatModelFallbacks("anthropic");
		const existing = { ...fallback, id: "claude-sonnet-5-5", name: "Existing model", custom: true };
		const { root, catalogPath, dataPath } = fixture([existing]);
		hydrateModelCatalog(root, catalogPath);
		validateGeneratedModelData(root);
		const first = readFileSync(dataPath, "utf8");
		const manifestPath = join(root, "src/providers/data", MODEL_DATA_MANIFEST_FILE);
		const manifest = readFileSync(manifestPath, "utf8");
		const data = JSON.parse(first) as Record<string, Record<string, unknown>>;
		expect(data["anthropic-messages"]).toEqual({
			"chat:claude-sonnet-5-5": existing,
			"chat:claude-haiku-5-5": fallback,
			"chat:claude-fable-5-2": fable,
		});
		hydrateModelCatalog(root, catalogPath);
		expect(readFileSync(dataPath, "utf8")).toBe(first);
		expect(readFileSync(manifestPath, "utf8")).toBe(manifest);
		expect(fetch).not.toHaveBeenCalled();
	});

	it("does not replace a supplied Haiku record or its provider-specific metadata", () => {
		const [fallback, fable] = getForkChatModelFallbacks("anthropic");
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
		expect(data["anthropic-messages"]).toEqual({
			"chat:claude-haiku-5-5": supplied,
			"chat:claude-fable-5-2": fable,
		});
	});

	it.each(["anthropic", "openrouter"])("hydrates missing %s Fable 5.2 with managed effort offline", (provider) => {
		const fetch = vi.fn(() => {
			throw new Error("Unexpected network request");
		});
		vi.stubGlobal("fetch", fetch);
		const fallback = getForkChatModelFallbacks(provider).find((model) => model.id.includes("fable"))!;
		const { root, catalogPath, dataPath } = fixture([{ ...fallback, id: "existing-chat" }], provider);
		hydrateModelCatalog(root, catalogPath);
		validateGeneratedModelData(root);
		const first = readFileSync(dataPath, "utf8");
		const data = JSON.parse(first) as Record<string, Record<string, unknown>>;
		expect(data["anthropic-messages"][`chat:${fallback.id}`]).toEqual(fallback);
		expect(fallback).toMatchObject({
			cost: { input: 8, output: 40, cacheRead: 0.2, cacheWrite: 10 },
			contextWindow: 1000000,
			maxTokens: 128000,
			compat: { forceAdaptiveThinking: true, supportsMidConvoEffort: true, supportsTemperature: false },
		});
		expect(getSupportedThinkingLevels(fallback)).toEqual(["low", "medium", "high", "xhigh", "max"]);
		if (provider === "anthropic") {
			expect(fallback.supportsFastMode).toBe(true);
			expect(fallback.compat).toMatchObject({
				supportsMidConvoSystemMessages: true,
				supportsMidConvoToolChanges: true,
				supportsStrictTools: true,
			});
			expect(fallback.compat?.allowedFallbackModels).toBeUndefined();
		} else {
			expect(fallback.baseUrl).toBe("https://openrouter.ai/api");
			expect(fallback.supportsFastMode).toBeUndefined();
			expect(fallback.compat?.allowedFallbackModels).toBeUndefined();
			expect(fallback.compat?.supportsMidConvoSystemMessages).toBeUndefined();
		}
		hydrateModelCatalog(root, catalogPath);
		expect(readFileSync(dataPath, "utf8")).toBe(first);
		expect(fetch).not.toHaveBeenCalled();
	});

	it.each(["anthropic", "openrouter"])("preserves a supplied %s Fable record without duplication", (provider) => {
		const fallback = getForkChatModelFallbacks(provider).find((model) => model.id.includes("fable"))!;
		const supplied = {
			...fallback,
			name: "Updated upstream Fable",
			cost: { ...fallback.cost, input: 9 },
			compat: { supportsMidConvoEffort: true, supportsStrictTools: false },
			custom: { preserve: true },
		};
		const { root, catalogPath, dataPath } = fixture([supplied], provider);
		hydrateModelCatalog(root, catalogPath);
		validateGeneratedModelData(root);
		const data = JSON.parse(readFileSync(dataPath, "utf8")) as Record<string, Record<string, unknown>>;
		expect(data["anthropic-messages"][`chat:${supplied.id}`]).toEqual(supplied);
		expect(Object.keys(data["anthropic-messages"])).toHaveLength(provider === "anthropic" ? 2 : 1);
	});

	it("hydrates only the selected Claude versions from an older pinned catalog", () => {
		const [haiku, fable] = getForkChatModelFallbacks("anthropic");
		const ids = [
			"claude-sonnet-5-5",
			"claude-opus-5-5",
			"claude-sonnet-4-5",
			"claude-sonnet-5",
			"claude-haiku-4-5",
			"claude-opus-4-8",
			"claude-opus-5",
			"claude-fable-5",
			"claude-fable-5-1",
		];
		const { root, catalogPath, dataPath } = fixture(ids.map((id) => ({ ...haiku, id })));
		hydrateModelCatalog(root, catalogPath);
		validateGeneratedModelData(root);
		const data = JSON.parse(readFileSync(dataPath, "utf8")) as Record<string, Record<string, unknown>>;
		expect(Object.keys(data["anthropic-messages"]).sort()).toEqual([
			"chat:claude-fable-5-2",
			"chat:claude-haiku-5-5",
			"chat:claude-opus-5-5",
			"chat:claude-sonnet-5-5",
		]);
		expect(data["anthropic-messages"]["chat:claude-fable-5-2"]).toEqual(fable);
		expect(fable).toMatchObject({
			cost: { input: 8, output: 40, cacheRead: 0.2, cacheWrite: 10 },
			contextWindow: 1000000,
			maxTokens: 128000,
			supportsFastMode: true,
			compat: {
				forceAdaptiveThinking: true,
				supportsMidConvoEffort: true,
				supportsMidConvoSystemMessages: true,
				supportsMidConvoToolChanges: true,
				supportsTemperature: false,
				supportsStrictTools: true,
			},
		});
		expect(getSupportedThinkingLevels(fable)).toEqual(["low", "medium", "high", "xhigh", "max"]);
	});

	it("preserves supplied Fable 5.2 metadata instead of replacing it with the fallback", () => {
		const [haiku, fable] = getForkChatModelFallbacks("anthropic");
		const supplied = { ...fable, name: "Updated upstream Fable", cost: { ...fable.cost, input: 9 } };
		const { root, catalogPath, dataPath } = fixture([supplied]);
		hydrateModelCatalog(root, catalogPath);
		const data = JSON.parse(readFileSync(dataPath, "utf8")) as Record<string, Record<string, unknown>>;
		expect(Object.values(data["anthropic-messages"])).toEqual([supplied, haiku]);
	});
});
