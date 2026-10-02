import { anthropicMessagesApi } from "../api/anthropic-messages.lazy.ts";
import { envApiKeyAuth, lazyOAuth } from "../auth/helpers.ts";
import { loadKimiCodingOAuth } from "../auth/oauth/load.ts";
import { createProvider, type Provider } from "../models.ts";
import { kimiCodingEndpoints, selectKimiCodingEndpoints } from "../utils/kimi-coding.ts";
import { KIMI_CODING_MODELS } from "./kimi-coding.models.ts";

export function kimiCodingProvider(): Provider<"anthropic-messages"> {
	const apiKeyAuth = envApiKeyAuth("Kimi API key", ["KIMI_API_KEY"]);
	return createProvider({
		id: "kimi-coding",
		name: "Kimi For Coding",
		baseUrl: "https://api.kimi.com/coding",
		auth: {
			apiKey: {
				...apiKeyAuth,
				async login(interaction) {
					const endpoints = await selectKimiCodingEndpoints(interaction);
					const credential = await apiKeyAuth.login!(interaction);
					return {
						...credential,
						env: { KIMI_CODE_OAUTH_HOST: endpoints.oauthHost, KIMI_CODE_BASE_URL: endpoints.baseUrl },
					};
				},
				async resolve(input) {
					const result = await apiKeyAuth.resolve(input);
					if (!result) return undefined;
					const oauthHost =
						input.credential?.env?.KIMI_CODE_OAUTH_HOST ||
						input.credential?.env?.KIMI_OAUTH_HOST ||
						(await input.ctx.env("KIMI_CODE_OAUTH_HOST")) ||
						(await input.ctx.env("KIMI_OAUTH_HOST"));
					const baseUrl = input.credential?.env?.KIMI_CODE_BASE_URL || (await input.ctx.env("KIMI_CODE_BASE_URL"));
					input.signal.throwIfAborted();
					if (!oauthHost && !baseUrl) return result;
					return {
						...result,
						auth: { ...result.auth, baseUrl: kimiCodingEndpoints({ oauthHost, baseUrl }).baseUrl },
					};
				},
			},
			oauth: lazyOAuth({
				name: "Kimi Code (subscription)",
				isSubscription: true,
				loginLabel: "Sign in with Kimi Code",
				load: loadKimiCodingOAuth,
			}),
		},
		models: Object.values(KIMI_CODING_MODELS),
		api: anthropicMessagesApi(),
	});
}
