import { mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { afterEach, describe, expect, it, vi } from "vitest";
import { applyForkModelMetadata, getForkImageModels } from "../scripts/fork-model-data.ts";
import { hydrateModelCatalog } from "../scripts/hydrate-model-catalog.ts";
import { MODEL_DATA_MANIFEST_FILE, validateGeneratedModelData } from "../scripts/model-data.ts";
import type { Api, ClassifierModel, Model } from "../src/types.ts";

const roots: string[] = [];

afterEach(() => {
	vi.unstubAllGlobals();
	for (const root of roots.splice(0)) rmSync(root, { recursive: true, force: true });
});

function chat(provider: string, id: string, api: Api = "openai-responses"): Model<Api> & { type: "chat" } {
	return {
		type: "chat",
		id,
		name: id,
		api,
		provider,
		baseUrl: "https://example.com/v1",
		reasoning: true,
		input: ["text"],
		cost: { input: 1, output: 2, cacheRead: 0.1, cacheWrite: 0 },
		contextWindow: 272000,
		maxTokens: 128000,
		compat: { supportsStrictMode: true },
	};
}

describe("fork model metadata", () => {
	it.each([
		["openai", "gpt-6-astra", "openai-responses"],
		["openai", "gpt-6.1-sol", "openai-responses"],
		["openai", "gpt-5.5", "openai-responses"],
		["cloudflare-ai-gateway", "gpt-6-sol", "openai-responses"],
		["openai-codex", "gpt-6-luna", "openai-codex-responses"],
		["anthropic", "claude-opus-5-5", "anthropic-messages"],
		["anthropic", "claude-sonnet-5", "anthropic-messages"],
	] as const)("keeps verified fast-mode support for %s/%s", (provider, id, api) => {
		const model = chat(provider, id, api);
		applyForkModelMetadata(model);
		expect(model.supportsFastMode).toBe(true);
	});

	it.each([
		["azure", "gpt-6-astra", "azure-openai-responses"],
		["github-copilot", "gpt-6-astra", "openai-responses"],
		["cloudflare-ai-gateway", "claude-opus-5-5", "anthropic-messages"],
		["openai", "gpt-6-unknown", "openai-responses"],
		["anthropic", "claude-sonnet-5-5", "anthropic-messages"],
		["anthropic", "claude-haiku-5-5", "anthropic-messages"],
	] as const)("does not infer fast-mode support for %s/%s", (provider, id, api) => {
		const model = chat(provider, id, api);
		applyForkModelMetadata(model);
		expect(model.supportsFastMode).toBeUndefined();
	});

	it("retains thinking exclusions without replacing unrelated levels or metadata", () => {
		const astra = chat("openai", "gpt-6-astra");
		astra.thinkingLevelMap = { off: "none", minimal: "low", low: "wrong" };
		astra.samplingParamsByThinkingLevel = { high: { temperature: 0.8 } };
		applyForkModelMetadata(astra);
		expect(astra.thinkingLevelMap).toEqual({
			off: null,
			minimal: null,
			low: "low",
			medium: "medium",
			high: "high",
			xhigh: "xhigh",
			max: "max",
		});
		expect(astra.compat).toEqual({ supportsStrictMode: true });
		expect(astra.samplingParamsByThinkingLevel).toEqual({ high: { temperature: 0.8 } });

		const opus = chat("openrouter", "anthropic/claude-opus-5-5", "anthropic-messages");
		opus.thinkingLevelMap = { off: "none", minimal: "low", high: "custom" };
		applyForkModelMetadata(opus);
		expect(opus.thinkingLevelMap).toEqual({ off: null, minimal: null, high: "custom" });

		for (const id of ["openai/gpt-6-astra", "openai/gpt-6.1-sol"]) {
			const proxy = chat("openrouter", id, "openai-completions");
			applyForkModelMetadata(proxy);
			expect(proxy.thinkingLevelMap).toEqual({ off: null, minimal: null });
		}
		for (const id of ["gpt-6-sol", "gpt-6-luna"]) {
			const direct = chat("openai", id);
			const codex = chat("openai-codex", id, "openai-codex-responses");
			codex.thinkingLevelMap = { minimal: "low" };
			applyForkModelMetadata(direct);
			applyForkModelMetadata(codex);
			expect(direct.thinkingLevelMap).toEqual({ minimal: null });
			expect(codex.thinkingLevelMap).toEqual({ minimal: "low" });
		}
		const sol61 = chat("openai-codex", "gpt-6.1-sol", "openai-codex-responses");
		sol61.thinkingLevelMap = { off: null, minimal: "low", high: "high", max: "max" };
		applyForkModelMetadata(sol61);
		expect(sol61.thinkingLevelMap).toEqual({ off: null, minimal: "low", high: "high", max: "max" });
	});

	it.each(["gpt-6-astra", "gpt-6-sol", "gpt-6-luna", "gpt-6.1-sol"])("expands only the Azure context of %s", (id) => {
		const azure = chat("azure", id, "azure-openai-responses");
		const completions = chat("azure", id, "openai-completions");
		const direct = chat("openai", id);
		applyForkModelMetadata(azure);
		applyForkModelMetadata(completions);
		applyForkModelMetadata(direct);
		expect(azure.provider).toBe("azure");
		expect(azure.api).toBe("azure-openai-responses");
		expect(azure.contextWindow).toBe(1050000);
		expect(azure.maxTokens).toBe(128000);
		expect(azure.cost).toEqual(direct.cost);
		expect(completions.contextWindow).toBe(272000);
		expect(direct.contextWindow).toBe(272000);
	});

	it("leaves unrelated models, unknown APIs, and other model types unchanged", () => {
		const models = [
			{ ...chat("openai", "future-model"), custom: { untouched: true } },
			chat("openai", "gpt-6-astra", "future-api"),
			{ type: "image", provider: "openai", id: "gpt-6-astra", api: "openai-responses" },
			{ type: "classifier", provider: "anthropic", id: "claude-opus-5-5", api: "anthropic-messages" },
			chat("azure", "constructor", "azure-openai-responses"),
			chat("azure", "gpt-6-astra", "azure"),
		];
		const before = structuredClone(models);
		for (const model of models) applyForkModelMetadata(model);
		expect(models).toEqual(before);
	});

	it("creates isolated Codex image entries with the existing image-input defaults", () => {
		const [model] = getForkImageModels("openai-codex");
		expect(model).toEqual({
			type: "image",
			id: "chatgpt-image-generation",
			name: "ChatGPT Image Generation",
			api: "openai-codex-images",
			provider: "openai-codex",
			baseUrl: "https://chatgpt.com/backend-api",
			input: ["text", "image"],
			output: ["image"],
			cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0 },
			inputLimits: { images: { resize: { maxWidth: 2000, maxHeight: 2000, maxBytes: 4718592, jpegQuality: 80 } } },
		});
		model.input.push("text");
		expect(getForkImageModels("openai-codex")[0].input).toEqual(["text", "image"]);
		expect(getForkImageModels("other")).toEqual([]);
	});

	it("hydrates upstream catalogs offline with deterministic fork entries and preserves existing image records", () => {
		const fetch = vi.fn(() => {
			throw new Error("Unexpected network request");
		});
		vi.stubGlobal("fetch", fetch);
		const root = mkdtempSync(join(tmpdir(), "pi-fork-model-data-"));
		roots.push(root);
		const providers = ["openai-codex", "openai", "azure", "anthropic"];
		const providersDir = join(root, "src/providers");
		mkdirSync(providersDir, { recursive: true });
		writeFileSync(
			join(root, "src/models.generated.ts"),
			providers
				.map((provider) => {
					const prefix = provider.toUpperCase().replace(/-/g, "_");
					writeFileSync(join(providersDir, `${provider}.models.ts`), "");
					return `import { ${prefix}_CLASSIFIER_MODELS, ${prefix}_IMAGE_MODELS, ${prefix}_MODELS } from "./providers/${provider}.models.ts";`;
				})
				.join("\n"),
		);
		const deepseek = chat("azure", "deepseek-v4-pro", "openai-completions");
		deepseek.contextWindow = 1000000;
		deepseek.thinkingLevelMap = { minimal: null, low: "low", medium: "medium", high: "high", xhigh: null, max: null };
		deepseek.compat = {
			supportsDeveloperRole: false,
			supportsMidConvoSystemMessages: true,
			thinkingFormat: "openai",
			supportsLongCacheRetention: false,
		};
		const classifier: ClassifierModel<"openai-decisions"> = {
			type: "classifier",
			id: "gpt-6-luna",
			name: "GPT-6 Luna",
			api: "openai-decisions",
			provider: "openai",
			baseUrl: "https://api.openai.com/v1",
			input: ["text", "image"],
			cost: {
				input: 0.1,
				output: 0,
				cacheRead: 0,
				cacheWrite: 0,
				tiers: [{ inputTokensAbove: 272000, input: 0.2, output: 0, cacheRead: 0, cacheWrite: 0 }],
			},
			contextWindow: 922000,
		};
		const catalog = {
			"openai-codex": [chat("openai-codex", "gpt-6-astra", "openai-codex-responses")],
			openai: [
				chat("openai", "gpt-6-astra"),
				chat("openai", "future-model", "future-api"),
				chat("openai", "gpt-6-luna"),
				classifier,
			],
			azure: [chat("azure", "gpt-6-sol", "azure-openai-responses"), deepseek],
			anthropic: [chat("anthropic", "claude-opus-5-5", "anthropic-messages")],
		};
		const path = join(root, "models.all.json");
		writeFileSync(path, JSON.stringify(catalog));
		hydrateModelCatalog(root, path);
		validateGeneratedModelData(root);
		const snapshot = [...providers.map((provider) => `${provider}.json`), MODEL_DATA_MANIFEST_FILE].map((file) =>
			readFileSync(join(providersDir, "data", file), "utf8"),
		);
		hydrateModelCatalog(root, path);
		expect(
			[...providers.map((provider) => `${provider}.json`), MODEL_DATA_MANIFEST_FILE].map((file) =>
				readFileSync(join(providersDir, "data", file), "utf8"),
			),
		).toEqual(snapshot);
		const codex = JSON.parse(snapshot[0]) as Record<string, Record<string, unknown>>;
		expect(codex["openai-codex-images"]["image:chatgpt-image-generation"]).toEqual(
			getForkImageModels("openai-codex")[0],
		);
		const openai = JSON.parse(snapshot[1]) as Record<string, Record<string, unknown>>;
		expect(openai["future-api"]["chat:future-model"]).toEqual(catalog.openai[1]);
		expect(openai["openai-responses"]["chat:gpt-6-astra"]).toMatchObject({
			supportsFastMode: true,
			thinkingLevelMap: { off: null, minimal: null },
		});
		expect(openai["openai-responses"]["chat:gpt-6-luna"]).toMatchObject({ supportsFastMode: true });
		expect(openai["openai-decisions"]["classifier:gpt-6-luna"]).toEqual(classifier);
		const azure = JSON.parse(snapshot[2]) as Record<string, Record<string, unknown>>;
		expect(Object.keys(azure).sort()).toEqual(["azure-openai-responses", "openai-completions"]);
		expect(azure["azure-openai-responses"]["chat:gpt-6-sol"]).toMatchObject({
			provider: "azure",
			api: "azure-openai-responses",
			contextWindow: 1050000,
			thinkingLevelMap: { minimal: null },
		});
		expect(azure["openai-completions"]["chat:deepseek-v4-pro"]).toEqual(deepseek);

		const existing = { ...getForkImageModels("openai-codex")[0], name: "Existing image", custom: true };
		writeFileSync(path, JSON.stringify({ ...catalog, "openai-codex": [...catalog["openai-codex"], existing] }));
		hydrateModelCatalog(root, path);
		const withExisting = JSON.parse(readFileSync(join(providersDir, "data/openai-codex.json"), "utf8")) as Record<
			string,
			Record<string, unknown>
		>;
		expect(Object.values(withExisting["openai-codex-images"])).toEqual([existing]);
		expect(fetch).not.toHaveBeenCalled();
	});
});
