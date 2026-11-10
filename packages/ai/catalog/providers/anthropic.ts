import { getForkChatModelFallbacks } from "../../scripts/fork-model-fallbacks.ts";
import type { Model } from "../../src/types.ts";
import { defineProvider, mergeCompat, mergeThinking, modelsDevModels, withInputLimits } from "../define.ts";
import {
	claude55ThinkingLevels,
	enableClaudeMidConvoEffort,
	supportsClaudeMidConvoEffort,
	supportsClaudeMidConvoSystemMessages,
} from "../families/anthropic.ts";

const MODELS_55 = new Set(["claude-opus-5-5", "claude-sonnet-5-5", "claude-haiku-5-5"]);

export default defineProvider({
	id: "anthropic",
	models: (ctx) => [
		...modelsDevModels(ctx, "anthropic", {
			api: "anthropic-messages",
			baseUrl: "https://api.anthropic.com",
			include: (model) => MODELS_55.has(model.id) || model.id === "claude-fable-5-2",
			map: (model) => {
				if (MODELS_55.has(model.id)) mergeThinking(model, claude55ThinkingLevels(model.id));
				return model;
			},
		}),
		{
			id: "claude-fable-5-2",
			name: "Claude Fable 5.2",
			api: "anthropic-messages",
			provider: "anthropic",
			baseUrl: "https://api.anthropic.com",
			reasoning: true,
			input: ["text", "image"],
			cost: { input: 8, output: 40, cacheRead: 0.2, cacheWrite: 10 },
			contextWindow: 1000000,
			maxTokens: 128000,
		},
		...getForkChatModelFallbacks("anthropic"),
	],
	finalize: (model) => {
		if (model.api !== "anthropic-messages") return;
		const messagesModel = model as Model<"anthropic-messages">;
		if (supportsClaudeMidConvoEffort(model.id)) enableClaudeMidConvoEffort(messagesModel);
		if (supportsClaudeMidConvoSystemMessages(model.id)) {
			mergeCompat(messagesModel, { supportsMidConvoSystemMessages: true, supportsMidConvoToolChanges: true });
		}
		mergeCompat(messagesModel, { supportsStrictTools: true });
		// Ephemeral entries have a hard five-minute lifetime; `ttl: "1h"` extends
		// it to one hour. Only direct Anthropic is annotated so cache warming does
		// not assume equivalent behavior through proxies.
		// https://docs.anthropic.com/en/docs/build-with-claude/prompt-caching
		model.promptCache = { short: 300, long: 3600 };
		withInputLimits(model, {
			maxRequestBytes: 32 * 1024 * 1024,
			images: { maxPerRequest: model.contextWindow === 200000 ? 100 : 600 },
		});
	},
});
