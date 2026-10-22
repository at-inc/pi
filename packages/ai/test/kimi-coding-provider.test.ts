import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { createModels } from "../src/models.ts";
import { kimiCodingProvider } from "../src/providers/kimi-coding.ts";
import type { ProviderEnv } from "../src/types.ts";

describe("Kimi Code API-key regions", () => {
	beforeEach(() => {
		for (const name of ["KIMI_API_KEY", "KIMI_CODE_OAUTH_HOST", "KIMI_OAUTH_HOST", "KIMI_CODE_BASE_URL"]) {
			vi.stubEnv(name, "");
		}
	});

	afterEach(() => vi.unstubAllEnvs());

	// #10720
	it.each([
		["global", "ai"],
		["mainland-cn", "com"],
	])("persists the %s region with an API key", async (region, domain) => {
		const models = createModels();
		models.setProvider(kimiCodingProvider());
		const prompts: string[] = [];
		const credential = await models.login("kimi-coding", "api_key", {
			prompt: async (prompt) => {
				prompts.push(prompt.type);
				return prompt.type === "select" ? region : "test-key";
			},
			notify: () => {},
		});
		expect(prompts).toEqual(["select", "secret"]);
		expect(credential).toEqual({
			type: "api_key",
			key: "test-key",
			env: {
				KIMI_CODE_OAUTH_HOST: `https://auth.kimi.${domain}`,
				KIMI_CODE_BASE_URL: `https://api.kimi.${domain}/coding/v1`,
			},
		});
		vi.stubEnv("KIMI_CODE_OAUTH_HOST", "https://auth.example.com");
		vi.stubEnv("KIMI_CODE_BASE_URL", "https://api.example.com");
		await expect(models.getAuth("kimi-coding")).resolves.toMatchObject({
			auth: { apiKey: "test-key", baseUrl: `https://api.kimi.${domain}/coding/v1` },
		});
		const request = vi.fn(async (input: string | URL | Request, init?: RequestInit) => {
			expect(input instanceof Request ? input.url : String(input)).toBe(
				`https://api.kimi.${domain}/coding/v1/chat/completions`,
			);
			expect(new Headers(init?.headers).get("authorization")).toBe("Bearer test-key");
			return new Response(
				'data: {"id":"chat_test","choices":[{"index":0,"delta":{"content":"Hello"},"finish_reason":"stop"}]}\n\ndata: [DONE]\n\n',
				{ headers: { "content-type": "text/event-stream" } },
			);
		});
		const result = await models.completeSimple(
			models.getModel("kimi-coding", "kimi-for-coding")!,
			{ messages: [{ role: "user", content: "Hello", timestamp: 0 }] },
			{ fetch: request },
		);
		expect(result.stopReason).toBe("stop");
		expect(result.errorMessage).toBeUndefined();
		expect(request).toHaveBeenCalledOnce();
	});

	it.each<ProviderEnv>([
		{ KIMI_CODE_BASE_URL: "https://api.kimi.ai/coding" },
		{ KIMI_CODE_BASE_URL: "https://api.kimi.ai/coding/v1/" },
		{ KIMI_CODE_OAUTH_HOST: "https://auth.kimi.ai" },
		{ KIMI_OAUTH_HOST: "https://auth.kimi.ai/" },
	])("resolves international endpoints from scoped configuration %j", async (env) => {
		const models = createModels();
		models.setProvider(kimiCodingProvider());
		await expect(models.getAuth("kimi-coding", { apiKey: "test-key", env })).resolves.toMatchObject({
			auth: { apiKey: "test-key", baseUrl: "https://api.kimi.ai/coding/v1" },
		});
	});

	it("resolves an ambient API key and custom endpoint", async () => {
		vi.stubEnv("KIMI_API_KEY", "env-key");
		vi.stubEnv("KIMI_CODE_BASE_URL", "https://gateway.example.com/kimi/v1/");
		const models = createModels();
		models.setProvider(kimiCodingProvider());
		await expect(models.getAuth("kimi-coding")).resolves.toMatchObject({
			auth: { apiKey: "env-key", baseUrl: "https://gateway.example.com/kimi/v1" },
		});
	});

	it("preserves the model endpoint when no regional override is configured", async () => {
		const models = createModels();
		models.setProvider(kimiCodingProvider());
		await expect(models.getAuth("kimi-coding", { apiKey: "test-key" })).resolves.toMatchObject({
			auth: { apiKey: "test-key" },
		});
		expect((await models.getAuth("kimi-coding", { apiKey: "test-key" }))?.auth.baseUrl).toBeUndefined();
		await expect(models.getAuth("kimi-coding")).resolves.toBeUndefined();
	});
});
