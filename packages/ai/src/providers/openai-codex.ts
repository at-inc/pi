import { openAICodexImagesApi } from "../api/openai-codex-images.lazy.ts";
import { openAICodexResponsesApi } from "../api/openai-codex-responses.lazy.ts";
import { lazyOAuth } from "../auth/helpers.ts";
import { loadOpenAICodexOAuth } from "../auth/oauth/load.ts";
import { createProvider, type Provider } from "../models.ts";
import { OPENAI_CODEX_IMAGE_MODELS, OPENAI_CODEX_MODELS } from "./openai-codex.models.ts";

export const OPENAI_CODEX_BASE_URL = "https://chatgpt.com/backend-api";

export function openaiCodexProvider(): Provider<"openai-codex-responses"> {
	return createProvider({
		id: "openai-codex",
		name: "OpenAI Codex (legacy)",
		baseUrl: OPENAI_CODEX_BASE_URL,
		auth: {
			oauth: lazyOAuth({
				name: "OpenAI (ChatGPT Plus/Pro)",
				isSubscription: true,
				load: loadOpenAICodexOAuth,
			}),
		},
		models: [...Object.values(OPENAI_CODEX_MODELS), ...Object.values(OPENAI_CODEX_IMAGE_MODELS)],
		api: openAICodexResponsesApi(),
		images: { "openai-codex-images": openAICodexImagesApi() },
	});
}
