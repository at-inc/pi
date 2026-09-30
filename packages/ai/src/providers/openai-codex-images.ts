import { openAICodexImagesApi } from "../api/openai-codex-images.lazy.ts";
import { createProvider, type Provider } from "../models.ts";
import { OPENAI_CODEX_IMAGE_MODELS } from "./openai-codex.models.ts";
import { openaiCodexProvider } from "./openai-codex.ts";

export function openaiCodexImagesProvider(): Provider {
	return createProvider({
		id: "openai-codex",
		name: "ChatGPT",
		auth: openaiCodexProvider().auth,
		models: Object.values(OPENAI_CODEX_IMAGE_MODELS),
		images: { "openai-codex-images": openAICodexImagesApi() },
	});
}
