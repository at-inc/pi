import type { Model } from "../src/types.ts";
import { applyForkModelMetadata, applyImageInputMetadata } from "./fork-model-data.ts";

export function getForkChatModelFallbacks(provider: string): (Model<"anthropic-messages"> & { type: "chat" })[] {
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
			off: null,
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
	return [model];
}
