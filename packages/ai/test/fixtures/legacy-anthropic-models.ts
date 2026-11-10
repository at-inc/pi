import type { Model } from "../../src/types.ts";

const legacyModels = {
	"claude-haiku-4-5": {
		name: "Claude Haiku 4.5 (latest)",
		cost: { input: 1, output: 5, cacheRead: 0.1, cacheWrite: 1.25 },
		contextWindow: 200000,
		maxTokens: 64000,
	},
	"claude-sonnet-4-5": {
		name: "Claude Sonnet 4.5 (latest)",
		cost: { input: 3, output: 15, cacheRead: 0.3, cacheWrite: 3.75 },
		contextWindow: 200000,
		maxTokens: 64000,
	},
	"claude-sonnet-4-6": {
		name: "Claude Sonnet 4.6",
		cost: { input: 3, output: 15, cacheRead: 0.3, cacheWrite: 3.75 },
		thinkingLevelMap: { max: "max" },
		compat: { forceAdaptiveThinking: true, supportsStrictTools: true },
	},
	"claude-sonnet-5": {
		name: "Claude Sonnet 5",
		cost: { input: 2, output: 10, cacheRead: 0.2, cacheWrite: 2.5 },
		thinkingLevelMap: { xhigh: "xhigh", max: "max" },
		compat: { forceAdaptiveThinking: true, supportsTemperature: false, supportsStrictTools: true },
	},
	"claude-opus-4-5": {
		name: "Claude Opus 4.5 (latest)",
		contextWindow: 200000,
		maxTokens: 64000,
	},
	"claude-opus-4-6": {
		name: "Claude Opus 4.6",
		thinkingLevelMap: { max: "max" },
		compat: { forceAdaptiveThinking: true, supportsStrictTools: true },
		supportsFastMode: true,
	},
	"claude-opus-4-7": {
		name: "Claude Opus 4.7",
		thinkingLevelMap: { xhigh: "xhigh", max: "max" },
		compat: { forceAdaptiveThinking: true, supportsTemperature: false, supportsStrictTools: true },
		supportsFastMode: true,
	},
	"claude-opus-4-8": {
		name: "Claude Opus 4.8",
		thinkingLevelMap: { xhigh: "xhigh", max: "max" },
		compat: {
			forceAdaptiveThinking: true,
			supportsTemperature: false,
			supportsMidConvoSystemMessages: true,
			supportsMidConvoToolChanges: true,
			supportsStrictTools: true,
		},
		supportsFastMode: true,
	},
	"claude-opus-5": {
		name: "Claude Opus 5",
		thinkingLevelMap: { xhigh: "xhigh", max: "max" },
		compat: {
			forceAdaptiveThinking: true,
			supportsTemperature: false,
			supportsMidConvoEffort: true,
			supportsMidConvoSystemMessages: true,
			supportsMidConvoToolChanges: true,
			supportsStrictTools: true,
		},
		supportsFastMode: true,
	},
	"claude-fable-5": {
		name: "Claude Fable 5",
		cost: { input: 10, output: 50, cacheRead: 1, cacheWrite: 12.5 },
		thinkingLevelMap: { off: null, xhigh: "xhigh", max: "max" },
		compat: {
			forceAdaptiveThinking: true,
			supportsMidConvoSystemMessages: true,
			supportsMidConvoToolChanges: true,
			supportsStrictTools: true,
			allowedFallbackModels: [
				{
					provider: "anthropic",
					model: "claude-opus-4-8",
					cost: { input: 5, output: 25, cacheRead: 0.5, cacheWrite: 6.25 },
				},
				{
					provider: "anthropic",
					model: "claude-opus-5",
					cost: { input: 5, output: 25, cacheRead: 0.5, cacheWrite: 6.25 },
				},
			],
		},
	},
	"claude-fable-5-1": {
		name: "Claude Fable 5.1",
		cost: { input: 10, output: 50, cacheRead: 0.25, cacheWrite: 12.5 },
		thinkingLevelMap: { off: null, xhigh: "xhigh", max: "max" },
		compat: {
			forceAdaptiveThinking: true,
			supportsMidConvoEffort: true,
			supportsMidConvoSystemMessages: true,
			supportsMidConvoToolChanges: true,
			supportsStrictTools: true,
		},
	},
} satisfies Record<string, Partial<Model<"anthropic-messages">>>;

/** Historical request fixtures, independent of which models the built-in catalog lists. */
export function getLegacyAnthropicModel(id: keyof typeof legacyModels): Model<"anthropic-messages"> {
	const model: Model<"anthropic-messages"> = {
		type: "chat",
		id,
		api: "anthropic-messages",
		provider: "anthropic",
		baseUrl: "https://api.anthropic.com",
		reasoning: true,
		input: ["text", "image"],
		cost: { input: 5, output: 25, cacheRead: 0.5, cacheWrite: 6.25 },
		contextWindow: 1000000,
		maxTokens: 128000,
		compat: { supportsStrictTools: true },
		promptCache: { short: 300, long: 3600 },
		supportsFastMode: false,
		...structuredClone(legacyModels[id]),
	};
	model.inputLimits = {
		maxRequestBytes: 32 * 1024 * 1024,
		images: {
			maxPerRequest: model.contextWindow === 200000 ? 100 : 600,
			resize: { maxWidth: 2000, maxHeight: 2000, maxBytes: 4.5 * 1024 * 1024, jpegQuality: 80 },
		},
	};
	return model;
}
