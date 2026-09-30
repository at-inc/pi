import type { ProviderImages } from "../types.ts";

export const openAICodexImagesApi = (): ProviderImages => ({
	generateImages: async (model, context, options) =>
		(await import("./openai-codex-images.ts")).generateImages(model, context, options),
});
