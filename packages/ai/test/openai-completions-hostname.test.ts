import { describe, expect, it, vi } from "vitest";
import { stream } from "../src/api/openai-completions.ts";
import { normalizeContext } from "../src/compat.ts";
import type { Model } from "../src/types.ts";

const model: Model<"openai-completions"> = {
	id: "hostname-test",
	name: "Hostname test",
	api: "openai-completions",
	provider: "custom",
	baseUrl: "https://example.test/v1",
	reasoning: false,
	input: ["text"],
	cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0 },
	contextWindow: 4096,
	maxTokens: 1024,
};

async function payload(baseUrl: string, overrides: Partial<typeof model> = {}) {
	let captured: Record<string, unknown> | undefined;
	const fetch = vi.fn(() => {
		throw new Error("Unexpected provider request");
	});
	const result = await stream(
		{ ...model, ...overrides, baseUrl },
		normalizeContext({ messages: [{ role: "user", content: "Offline hostname test", timestamp: 0 }] }),
		{
			apiKey: "test-key",
			fetch,
			maxRetries: 0,
			maxTokens: 16,
			cacheRetention: "short",
			sessionId: "hostname-session",
			onPayload(value) {
				captured = value as Record<string, unknown>;
				throw new Error("Payload captured before network request");
			},
		},
	).result();
	expect(result.stopReason).toBe("error");
	expect(result.errorMessage).toContain("Payload captured before network request");
	expect(fetch).not.toHaveBeenCalled();
	if (!captured) throw new Error("No request payload captured");
	return captured;
}

describe("OpenAI Completions endpoint hostname inference", () => {
	it.each([
		"api.cloudflare.com",
		"gateway.ai.cloudflare.com",
		"integrate.api.nvidia.com",
		"api.ant-ling.com",
		"api.deepseek.com",
		"api.together.ai",
		"api.together.xyz",
		"api.z.ai",
		"open.bigmodel.cn",
		"api.moonshot.ai",
		"api.moonshot.cn",
		"api.cerebras.ai",
		"chutes.ai",
		"opencode.ai",
		"api.x.ai",
	])("recognizes the real %s endpoint without accepting URL lookalikes", async (host) => {
		const official = await payload(`https://${host}/v1`);
		expect(official).not.toHaveProperty("store");
		for (const lookalike of [
			`https://${host}.example.test/v1`,
			`https://example.test/v1/${host}`,
			`https://example.test/v1?endpoint=${host}`,
			`https://example.test/v1#${host}`,
			`https://${host}@example.test/v1`,
		]) {
			const generic = await payload(lookalike);
			expect(generic.store).toBe(false);
			expect(generic.max_completion_tokens).toBe(16);
			expect(generic).not.toHaveProperty("max_tokens");
		}
	});

	it("only enables the official OpenAI cache key for its hostname", async () => {
		expect((await payload("https://api.openai.com/v1")).prompt_cache_key).toBe("hostname-session");
		expect((await payload("https://API.OPENAI.COM/v1")).prompt_cache_key).toBe("hostname-session");
		for (const url of [
			"https://api.openai.com.example.test/v1",
			"https://example.test/api.openai.com/v1",
			"https://example.test/v1?api.openai.com",
			"https://api.openai.com@example.test/v1",
		]) {
			expect((await payload(url)).prompt_cache_key).toBeUndefined();
		}
	});

	it("keeps OpenRouter domain inference separate from lookalike URLs", async () => {
		for (const url of ["https://openrouter.ai/api/v1", "https://proxy.openrouter.ai/v1"]) {
			expect((await payload(url, { reasoning: true })).reasoning).toEqual({ effort: "none" });
		}
		for (const url of [
			"https://openrouter.ai.example.test/v1",
			"https://example.test/v1?openrouter.ai",
			"https://openrouter.ai@example.test/v1",
		]) {
			expect((await payload(url, { reasoning: true })).reasoning).toBeUndefined();
		}
	});

	it("preserves explicit provider identity and compatibility overrides", async () => {
		const named = await payload("https://example.test/v1", { provider: "nvidia" });
		expect(named).not.toHaveProperty("store");
		expect(named.max_tokens).toBe(16);
		const explicit = await payload("https://integrate.api.nvidia.com/v1", {
			compat: { supportsStore: true, maxTokensField: "max_completion_tokens" },
		});
		expect(explicit.store).toBe(false);
		expect(explicit.max_completion_tokens).toBe(16);
	});
});
