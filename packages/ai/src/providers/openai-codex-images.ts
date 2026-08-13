import { openAICodexImagesApi } from "../api/openai-codex-images.lazy.ts";
import { createImagesProvider, type ImagesProvider } from "../images-models.ts";
import type { ImagesModel } from "../types.ts";
import { OPENAI_CODEX_BASE_URL, openaiCodexProvider } from "./openai-codex.ts";

const CHATGPT_IMAGE_GENERATION_MODEL = {
	id: "chatgpt-image-generation",
	name: "ChatGPT Image Generation",
	api: "openai-codex-images",
	provider: "openai-codex",
	baseUrl: OPENAI_CODEX_BASE_URL,
	input: ["text", "image"],
	output: ["image"],
	cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0 },
} satisfies ImagesModel<"openai-codex-images">;

export function openaiCodexImagesProvider(): ImagesProvider {
	return createImagesProvider({
		id: "openai-codex",
		name: "ChatGPT",
		auth: openaiCodexProvider().auth,
		models: [CHATGPT_IMAGE_GENERATION_MODEL],
		api: openAICodexImagesApi(),
	});
}
