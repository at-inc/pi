const JWT_CLAIM_PATH = "https://api.openai.com/auth";

/** Reads the ChatGPT account ID claim of an OpenAI access or ID token; undefined when the token has none. */
export function getChatGPTAccountId(token: string | undefined): string | undefined {
	const parts = token?.split(".");
	if (parts?.length !== 3) return undefined;
	try {
		// JWT payloads are base64url; atob rejects its "-" and "_", which any non-ASCII claim can produce.
		const payload = JSON.parse(atob(parts[1].replace(/-/g, "+").replace(/_/g, "/")));
		const accountId = payload?.[JWT_CLAIM_PATH]?.chatgpt_account_id;
		return typeof accountId === "string" && accountId.length > 0 ? accountId : undefined;
	} catch {
		return undefined;
	}
}
