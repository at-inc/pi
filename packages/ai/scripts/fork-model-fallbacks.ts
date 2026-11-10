import type { Model } from "../src/types.ts";
import { applyForkModelMetadata, applyImageInputMetadata } from "./fork-model-data.ts";

export function getForkChatModelFallbacks(provider: string): (Model<"anthropic-messages"> & { type: "chat" })[] {
	if (provider !== "anthropic") return [];
	const haiku: Model<"anthropic-messages"> & { type: "chat" } = {
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
	const fable: Model<"anthropic-messages"> & { type: "chat" } = {
		type: "chat",
		id: "claude-fable-5-2",
		name: "Claude Fable 5.2",
		api: "anthropic-messages",
		provider: "anthropic",
		baseUrl: "https://api.anthropic.com",
		reasoning: true,
		thinkingLevelMap: {
			off: null,
			minimal: null,
			low: "low",
			medium: "medium",
			high: "high",
			xhigh: "xhigh",
			max: "max",
		},
		input: ["text", "image"],
		cost: { input: 8, output: 40, cacheRead: 0.2, cacheWrite: 10 },
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
	const models = [haiku, fable];
	for (const model of models) {
		applyForkModelMetadata(model);
		applyImageInputMetadata(model);
	}
	return models;
}
