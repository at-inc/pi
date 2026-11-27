import { buildCatalog } from "../catalog/build.ts";
import type { ProviderContext } from "../catalog/define.ts";
import { applyClaudeFamilyRules, enableClaudeMidConvoEffort } from "../catalog/families/anthropic.ts";
import azure from "../catalog/providers/azure.ts";
import deepseek from "../catalog/providers/deepseek.ts";
import openai from "../catalog/providers/openai.ts";
import openaiCodex from "../catalog/providers/openai-codex.ts";
import type { CatalogSources } from "../catalog/sources/index.ts";
import type { Api, Model } from "../src/types.ts";
import { applyForkModelMetadata, applyImageInputMetadata } from "./fork-model-data.ts";

// Fallback models are authored here, so no upstream source backs them.
const EMPTY_SOURCES: CatalogSources = {
	modelsDev: {},
	modelsDevDecisions: {},
	openRouter: { chat: [], images: [], classifiers: [] },
	aiGateway: { chat: [], classifiers: [] },
	nvidiaModelIds: new Map(),
	radius: { chat: [], images: [], classifiers: [] },
};

// Fable 5.2 and Mythos 5.2 are the same model with the same specifications and pricing; only access differs.
// https://platform.claude.com/docs/en/models/mythos-5-2/overview
function getClaude52Fallback(
	provider: string,
	family: "Fable" | "Mythos",
): Model<"anthropic-messages"> & { type: "chat" } {
	const slug = family.toLowerCase();
	const model: Model<"anthropic-messages"> & { type: "chat" } = {
		type: "chat",
		id: provider === "anthropic" ? `claude-${slug}-5-2` : `anthropic/claude-${slug}-5.2`,
		name: provider === "anthropic" ? `Claude ${family} 5.2` : `Anthropic: Claude ${family} 5.2`,
		api: "anthropic-messages",
		provider,
		baseUrl: provider === "anthropic" ? "https://api.anthropic.com" : "https://openrouter.ai/api",
		reasoning: true,
		input: ["text", "image"],
		cost: { input: 8, output: 40, cacheRead: 0.2, cacheWrite: 10 },
		contextWindow: 1000000,
		maxTokens: 128000,
	};
	const context: ProviderContext = {
		providerId: provider,
		sources: EMPTY_SOURCES,
		strict: false,
		providerModels: () => [],
		modelsDevSource: () => undefined,
		recordModelsDevSource: () => {},
	};
	applyClaudeFamilyRules(model, context);
	enableClaudeMidConvoEffort(model);
	if (provider === "anthropic") {
		model.compat = {
			...model.compat,
			supportsMidConvoSystemMessages: true,
			supportsMidConvoToolChanges: true,
			supportsStrictTools: true,
		};
		model.promptCache = { short: 300, long: 3600 };
	}
	applyForkModelMetadata(model);
	applyImageInputMetadata(model);
	return model;
}

export function getForkChatModelFallbacks(provider: string): (Model<"anthropic-messages"> & { type: "chat" })[] {
	if (provider === "openrouter") return [getClaude52Fallback(provider, "Fable")];
	if (provider !== "anthropic") return [];
	const model: Model<"anthropic-messages"> & { type: "chat" } = {
		type: "chat",
		id: "claude-haiku-5-5",
		name: "Claude Haiku 5.5",
		api: "anthropic-messages",
		provider: "anthropic",
		baseUrl: "https://api.anthropic.com",
		reasoning: true,
		thinkingLevelMap: {
			minimal: null,
			low: "low",
			medium: "medium",
			high: "high",
			xhigh: "xhigh",
			max: "max",
		},
		input: ["text", "image"],
		cost: {
			input: 0.1,
			output: 0.5,
			cacheRead: 0.01,
			cacheWrite: 0.125,
			tiers: [{ inputTokensAbove: 100000, input: 0.5, output: 2.5, cacheRead: 0.05, cacheWrite: 0.625 }],
		},
		contextWindow: 1000000,
		maxTokens: 128000,
		compat: {
			supportsMidConvoEffort: true,
			supportsMidConvoSystemMessages: true,
			supportsMidConvoToolChanges: true,
			forceAdaptiveThinking: true,
			supportsTemperature: false,
			supportsStrictTools: true,
		},
		promptCache: { short: 300, long: 3600 },
	};
	applyForkModelMetadata(model);
	applyImageInputMetadata(model);
	return [model, getClaude52Fallback(provider, "Fable"), getClaude52Fallback(provider, "Mythos")];
}

// pi.dev's published catalog predates GPT-6.2, so hydrated data gets these models from the local catalog definitions.
const OPENAI_FALLBACK_MODEL_IDS = new Set(["gpt-6.2-astra", "gpt-6.2-sol"]);

export function getForkOpenAiChatModelFallbacks(provider: string): (Model<Api> & { type: "chat" })[] {
	if (provider !== "openai" && provider !== "azure" && provider !== "openai-codex") return [];
	const catalog = buildCatalog([azure, deepseek, openai, openaiCodex], EMPTY_SOURCES, false);
	return catalog[provider].chat.filter((model) => OPENAI_FALLBACK_MODEL_IDS.has(model.id));
}
