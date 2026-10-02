import type { ProviderAuthInteraction } from "../auth/types.ts";
import { getProviderEnvValue } from "./provider-env.ts";

/** Keep device authorization and Anthropic-compatible requests in the same Kimi deployment. */
export function kimiCodingEndpoints(input: { oauthHost?: string; baseUrl?: string }): {
	oauthHost: string;
	baseUrl: string;
} {
	const global =
		(input.oauthHost && new URL(input.oauthHost).hostname === "auth.kimi.ai") ||
		(!input.oauthHost && input.baseUrl && new URL(input.baseUrl).hostname === "api.kimi.ai");
	const domain = global ? "kimi.ai" : "kimi.com";
	return {
		oauthHost: (input.oauthHost || `https://auth.${domain}`).replace(/\/+$/, ""),
		// The Anthropic transport appends /v1/messages itself.
		baseUrl: (input.baseUrl || `https://api.${domain}/coding`).replace(/\/+$/, "").replace(/\/v1$/, ""),
	};
}

/** Select and persist endpoints at login so later refreshes cannot switch account regions. */
export async function selectKimiCodingEndpoints(interaction: ProviderAuthInteraction) {
	interaction.signal.throwIfAborted();
	const oauthHost = getProviderEnvValue("KIMI_CODE_OAUTH_HOST") || getProviderEnvValue("KIMI_OAUTH_HOST");
	const baseUrl = getProviderEnvValue("KIMI_CODE_BASE_URL");
	if (oauthHost || baseUrl) return kimiCodingEndpoints({ oauthHost, baseUrl });

	const region = await interaction.prompt({
		type: "select",
		message: "Select your Kimi account region:",
		options: [
			{ id: "global", label: "International (kimi.ai)", description: "Includes Google sign-in" },
			{ id: "mainland-cn", label: "Mainland China (kimi.com)" },
		],
	});
	interaction.signal.throwIfAborted();
	if (region !== "global" && region !== "mainland-cn") throw new Error(`Unknown Kimi account region: ${region}`);
	return kimiCodingEndpoints({ oauthHost: `https://auth.kimi.${region === "global" ? "ai" : "com"}` });
}
