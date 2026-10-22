import { describe, expect, it } from "vitest";
import { kimiCodingOAuth } from "../src/auth/oauth/kimi-coding.ts";
import { kimiCodingEndpoints } from "../src/utils/kimi-coding.ts";

// #10720
describe("Kimi Chat Completions endpoints", () => {
	it.each(["", "/", "/v1", "/v1/", "/v1/v1/"])("normalizes the inference suffix %j once", (suffix) => {
		expect(kimiCodingEndpoints({ baseUrl: `https://api.kimi.ai/coding${suffix}` })).toEqual({
			oauthHost: "https://auth.kimi.ai",
			baseUrl: "https://api.kimi.ai/coding/v1",
		});
	});

	it.each(["ai", "com"])("uses the stored %s deployment for OAuth inference", async (domain) => {
		await expect(
			kimiCodingOAuth.toAuth({
				type: "oauth",
				access: "test-access",
				refresh: "test-refresh",
				expires: 0,
				oauthHost: `https://auth.kimi.${domain}`,
				baseUrl: `https://api.kimi.${domain}/coding`,
			}),
		).resolves.toEqual({
			headers: { Authorization: "Bearer test-access" },
			baseUrl: `https://api.kimi.${domain}/coding/v1`,
		});
	});

	it("does not infer the international deployment from a lookalike hostname", () => {
		expect(kimiCodingEndpoints({ baseUrl: "https://api.kimi.ai.example/coding" })).toEqual({
			oauthHost: "https://auth.kimi.com",
			baseUrl: "https://api.kimi.ai.example/coding/v1",
		});
	});
});
