import { buildCatalog } from "../catalog/build.ts";
import { applyClaudeFamilyRules, enableClaudeMidConvoEffort } from "../catalog/families/anthropic.ts";
import azure from "../catalog/providers/azure.ts";
import deepseek from "../catalog/providers/deepseek.ts";
import openai from "../catalog/providers/openai.ts";
import openaiCodex from "../catalog/providers/openai-codex.ts";
import type { Api, Model } from "../src/types.ts";
import { applyForkModelMetadata, applyImageInputMetadata } from "./fork-model-data.ts";

export function getForkChatModelFallbacks(provider: string): (Model<"anthropic-messages"> & { type: "chat" })[] {
	if (provider !== "anthropic" && provider !== "openrouter") return [];
	const fable: Model<"anthropic-messages"> & { type: "chat" } = {
		type: "chat",
		id: provider === "anthropic" ? "claude-fable-5-2" : "anthropic/claude-fable-5.2",
		name: provider === "anthropic" ? "Claude Fable 5.2" : "Anthropic: Claude Fable 5.2",
		api: "anthropic-messages",
		provider,
		baseUrl: provider === "anthropic" ? "https://api.anthropic.com" : "https://openrouter.ai/api",
		reasoning: true,
		input: ["text", "image"],
		cost: { input: 8, output: 40, cacheRead: 0.2, cacheWrite: 10 },
		contextWindow: 1000000,
		maxTokens: 128000,
	};
	applyClaudeFamilyRules(fable);
	enableClaudeMidConvoEffort(fable);
	if (provider === "anthropic") {
		fable.compat = {
			...fable.compat,
			supportsMidConvoSystemMessages: true,
			supportsMidConvoToolChanges: true,
			supportsStrictTools: true,
		};
		fable.promptCache = { short: 300, long: 3600 };
	}
	applyForkModelMetadata(fable);
	applyImageInputMetadata(fable);
	if (provider === "openrouter") return [fable];
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
	return [model, fable];
}

// pi.dev's published catalog predates GPT-6.2, so hydrated data gets these models from the local catalog definitions.
const OPENAI_FALLBACK_MODEL_IDS = new Set(["gpt-6.2-astra", "gpt-6.2-sol"]);

export function getForkOpenAiChatModelFallbacks(provider: string): (Model<Api> & { type: "chat" })[] {
	if (provider !== "openai" && provider !== "azure" && provider !== "openai-codex") return [];
	const catalog = buildCatalog(
		[azure, deepseek, openai, openaiCodex],
		{
			modelsDev: {},
			modelsDevDecisions: {},
			openRouter: { chat: [], images: [], classifiers: [] },
			aiGateway: { chat: [], classifiers: [] },
			nvidiaModelIds: new Map(),
			radius: [],
		},
		false,
	);
	return catalog[provider].chat.filter((model) => OPENAI_FALLBACK_MODEL_IDS.has(model.id));
}
