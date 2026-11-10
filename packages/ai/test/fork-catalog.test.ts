import { describe, expect, it } from "vitest";
import { buildCatalog } from "../catalog/build.ts";
import { PROVIDERS } from "../catalog/providers/index.ts";
import type { CatalogSources } from "../catalog/sources/index.ts";
import type { ModelsDevV2Model } from "../catalog/sources/models-dev.ts";
import { getForkImageModels } from "../scripts/fork-model-data.ts";
import { getForkChatModelFallbacks } from "../scripts/fork-model-fallbacks.ts";

function sourceModel(id: string, protocol: string): ModelsDevV2Model {
	return {
		id,
		type: "chat",
		name: id,
		modalities: { input: ["text", "image"], output: ["text"] },
		capabilities: { tools: { supported: true }, reasoning: { supported: true } },
		api: { [protocol]: { base_url: "https://example.invalid/v1" } },
	};
}

const sources: CatalogSources = {
	modelsDev: {
		openai: {
			models: Object.fromEntries(
				[
					"gpt-5.3",
					"gpt-5.4",
					"gpt-5.10",
					"gpt-6-unknown",
					"gpt-10",
					"gpt-6-astra",
					"gpt-6-sol",
					"gpt-6-luna",
					"gpt-6.1-sol",
				].map((id) => [id, sourceModel(id, "responses")]),
			),
		},
		anthropic: {
			models: Object.fromEntries(
				["claude-opus-4-5", "claude-opus-4-6", "claude-opus-5-5", "claude-sonnet-5-5"].map((id) => [
					id,
					sourceModel(id, "messages"),
				]),
			),
		},
		"cloudflare-ai-gateway": {
			models: { "openai/gpt-6.1-terra": sourceModel("openai/gpt-6.1-terra", "responses") },
		},
	},
	modelsDevDecisions: {},
	openRouter: { chat: [], images: [], classifiers: [] },
	aiGateway: { chat: [], classifiers: [] },
	nvidiaModelIds: new Map(),
	radius: [],
};

describe("fork catalog integration", () => {
	it("applies version-based fast mode after shared and provider rules", () => {
		const catalog = buildCatalog(PROVIDERS, sources, false);
		for (const [provider, id, expected] of [
			["openai", "gpt-5.3", false],
			["openai", "gpt-5.4", true],
			["openai", "gpt-5.10", true],
			["openai", "gpt-6-unknown", true],
			["openai", "gpt-10", true],
			["anthropic", "claude-haiku-5-5", false],
			["anthropic", "claude-fable-5-2", true],
			["anthropic", "claude-opus-5-5", true],
			["anthropic", "claude-sonnet-5-5", false],
			["cloudflare-ai-gateway", "gpt-6.1-terra", true],
			["openai-codex", "gpt-6.1-terra", true],
			["azure", "gpt-6-astra", undefined],
		] as const) {
			const model = catalog[provider].chat.find((model) => model.id === id);
			expect(model, `${provider}/${id}`).toBeDefined();
			expect(model?.supportsFastMode, `${provider}/${id}`).toBe(expected);
		}
	});

	it("preserves fork thinking exclusions and Azure limits after provider finalization", () => {
		const catalog = buildCatalog(PROVIDERS, sources, false);
		expect(catalog["openai-codex"].chat.find((model) => model.id === "gpt-6-astra")).toMatchObject({
			thinkingLevelMap: { off: null, minimal: null, low: "low", max: "max" },
		});
		expect(catalog["openai-codex"].chat.find((model) => model.id === "gpt-6.1-sol")).toMatchObject({
			thinkingLevelMap: { off: null, minimal: "low" },
		});
		for (const id of ["gpt-6-astra", "gpt-6-sol", "gpt-6-luna", "gpt-6.1-sol"]) {
			expect(catalog.azure.chat.find((model) => model.id === id)).toMatchObject({ contextWindow: 1050000 });
		}
	});

	it("includes the native Codex image model and the complete Haiku fallback", () => {
		const catalog = buildCatalog(PROVIDERS, sources, false);
		expect(catalog["openai-codex"].image).toEqual(getForkImageModels("openai-codex"));
		expect(catalog.anthropic.chat.find((model) => model.id === "claude-haiku-5-5")).toEqual(
			getForkChatModelFallbacks("anthropic")[0],
		);
	});

	it("keeps upstream Haiku entries ahead of the fallback without duplicates", () => {
		const supplied = sourceModel("claude-haiku-5-5", "messages");
		supplied.name = "Updated upstream Haiku";
		supplied.limit = { context: 2000000, output: 128000 };
		const catalog = buildCatalog(
			PROVIDERS,
			{
				...sources,
				modelsDev: { ...sources.modelsDev, anthropic: { models: { [supplied.id]: supplied } } },
			},
			false,
		);
		const matches = catalog.anthropic.chat.filter((model) => model.id === supplied.id);
		expect(matches).toHaveLength(1);
		expect(matches[0]).toMatchObject({ name: supplied.name, contextWindow: 2000000 });
	});

	it.each(["anthropic", "openrouter"])("shares complete %s Fable metadata with pinned hydration", (provider) => {
		const catalog = buildCatalog(PROVIDERS, sources, false);
		const fallback = getForkChatModelFallbacks(provider).find((model) => model.id.includes("fable"))!;
		expect(catalog[provider].chat.filter((model) => model.id === fallback.id)).toEqual([fallback]);
	});

	it("includes Fable 5.2 with official pricing, limits, and always-on adaptive thinking", () => {
		const catalog = buildCatalog(PROVIDERS, sources, false);
		const matches = catalog.anthropic.chat.filter((model) => model.id === "claude-fable-5-2");
		expect(matches).toHaveLength(1);
		expect(matches[0]).toMatchObject({
			name: "Claude Fable 5.2",
			api: "anthropic-messages",
			provider: "anthropic",
			baseUrl: "https://api.anthropic.com",
			reasoning: true,
			input: ["text", "image"],
			cost: { input: 8, output: 40, cacheRead: 0.2, cacheWrite: 10 },
			contextWindow: 1000000,
			maxTokens: 128000,
			thinkingLevelMap: {
				off: null,
				minimal: null,
				low: "low",
				medium: "medium",
				high: "high",
				xhigh: "xhigh",
				max: "max",
			},
			supportsFastMode: true,
			compat: {
				forceAdaptiveThinking: true,
				supportsTemperature: false,
				supportsMidConvoEffort: true,
				supportsMidConvoSystemMessages: true,
				supportsMidConvoToolChanges: true,
				supportsStrictTools: true,
			},
			promptCache: { short: 300, long: 3600 },
		});
	});

	it("keeps upstream Fable 5.2 metadata without losing its capability rules or adding duplicates", () => {
		const supplied = sourceModel("claude-fable-5-2", "messages");
		supplied.name = "Updated upstream Fable";
		supplied.limit = { context: 1000000, output: 128000 };
		const catalog = buildCatalog(
			PROVIDERS,
			{
				...sources,
				modelsDev: { ...sources.modelsDev, anthropic: { models: { [supplied.id]: supplied } } },
			},
			false,
		);
		const matches = catalog.anthropic.chat.filter((model) => model.id === supplied.id);
		expect(matches).toHaveLength(1);
		expect(matches[0]).toMatchObject({
			name: supplied.name,
			thinkingLevelMap: {
				off: null,
				minimal: null,
				low: "low",
				medium: "medium",
				high: "high",
				xhigh: "xhigh",
				max: "max",
			},
			supportsFastMode: true,
			compat: { forceAdaptiveThinking: true, supportsTemperature: false, supportsMidConvoEffort: true },
		});
	});
});
