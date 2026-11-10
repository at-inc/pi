import { getForkChatModelFallbacks } from "../../scripts/fork-model-fallbacks.ts";
import type { Model } from "../../src/types.ts";
import { addMissing, defineProvider, mergeCompat, mergeThinking, modelsDevModels, withInputLimits } from "../define.ts";
import {
	claude55ThinkingLevels,
	enableClaudeMidConvoEffort,
	supportsClaudeMidConvoEffort,
	supportsClaudeMidConvoSystemMessages,
} from "../families/anthropic.ts";

const MODELS_55 = new Set(["claude-opus-5-5", "claude-sonnet-5-5", "claude-haiku-5-5"]);

export const ANTHROPIC_CATALOG_MODEL_IDS = new Set([...MODELS_55, "claude-fable-5-2"]);

export default defineProvider({
	id: "anthropic",
	models: (ctx) =>
		addMissing(
			modelsDevModels(ctx, "anthropic", {
				api: "anthropic-messages",
				baseUrl: "https://api.anthropic.com",
				include: (model) => ANTHROPIC_CATALOG_MODEL_IDS.has(model.id),
				map: (model) => {
					if (MODELS_55.has(model.id)) mergeThinking(model, claude55ThinkingLevels(model.id));
					return model;
				},
			}),
			getForkChatModelFallbacks("anthropic"),
		),
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
