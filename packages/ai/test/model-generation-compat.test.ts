import { spawnSync } from "node:child_process";
import { mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { afterEach, describe, expect, it } from "vitest";
import type { Model, OpenAICompletionsCompat } from "../src/types.ts";

const temporaryRoots: string[] = [];
const generatorUrl = new URL("../scripts/generate-models.ts", import.meta.url).href;
const endpoints: { hostname: string; provider?: string; expected: OpenAICompletionsCompat }[] = [
	{ hostname: "api.z.ai", provider: "zai", expected: { thinkingFormat: "zai", supportsReasoningEffort: false } },
	{
		hostname: "open.bigmodel.cn",
		provider: "zai-coding-cn",
		expected: { thinkingFormat: "zai", supportsReasoningEffort: false },
	},
	{
		hostname: "api.together.ai",
		provider: "together",
		expected: { thinkingFormat: "together", supportsLongCacheRetention: false },
	},
	{
		hostname: "api.together.xyz",
		provider: "together",
		expected: { thinkingFormat: "together", supportsLongCacheRetention: false },
	},
	{
		hostname: "api.moonshot.ai",
		provider: "moonshotai",
		expected: { maxTokensField: "max_tokens", supportsReasoningEffort: false },
	},
	{
		hostname: "api.moonshot.cn",
		provider: "moonshotai-cn",
		expected: { maxTokensField: "max_tokens", supportsReasoningEffort: false },
	},
	{
		hostname: "openrouter.ai",
		provider: "openrouter",
		expected: { thinkingFormat: "openrouter", sendSessionAffinityHeaders: true },
	},
	{
		hostname: "api.cloudflare.com",
		provider: "cloudflare-workers-ai",
		expected: { supportsStore: false, supportsLongCacheRetention: false },
	},
	{
		hostname: "gateway.ai.cloudflare.com",
		provider: "cloudflare-ai-gateway",
		expected: { maxTokensField: "max_tokens", supportsLongCacheRetention: false, supportsStrictMode: false },
	},
	{
		hostname: "integrate.api.nvidia.com",
		provider: "nvidia",
		expected: { maxTokensField: "max_tokens", supportsLongCacheRetention: false, supportsStrictMode: false },
	},
	{
		hostname: "api.ant-ling.com",
		provider: "ant-ling",
		expected: { thinkingFormat: "ant-ling", supportsLongCacheRetention: false },
	},
	{
		hostname: "cerebras.ai",
		provider: "cerebras",
		expected: { supportsStore: false, supportsStrictMode: false },
	},
	{
		hostname: "deepseek.com",
		provider: "deepseek",
		expected: { thinkingFormat: "deepseek", requiresReasoningContentOnAssistantMessages: true },
	},
	{ hostname: "api.x.ai", provider: "xai", expected: { supportsStore: false, supportsReasoningEffort: false } },
	{ hostname: "chutes.ai", expected: { supportsStore: false, maxTokensField: "max_tokens" } },
	{ hostname: "opencode.ai", provider: "opencode", expected: { supportsStore: false, supportsDeveloperRole: false } },
];

afterEach(() => {
	for (const root of temporaryRoots.splice(0)) rmSync(root, { recursive: true, force: true });
});

function detectCompat(inputs: { baseUrl: string; provider?: string }[]): OpenAICompletionsCompat[] {
	const root = mkdtempSync(join(tmpdir(), "pi-model-compat-"));
	temporaryRoots.push(root);
	const script = join(root, "detect.mjs");
	const models: Model<"openai-completions">[] = inputs.map(({ baseUrl, provider = "custom" }) => ({
		id: "test-model",
		name: "Test model",
		api: "openai-completions",
		provider,
		baseUrl,
		reasoning: true,
		input: ["text"],
		cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0 },
		contextWindow: 4096,
		maxTokens: 1024,
	}));
	writeFileSync(
		script,
		`import { detectOpenAICompletionsCompat } from ${JSON.stringify(generatorUrl)};\n` +
			`console.log(JSON.stringify(${JSON.stringify(models)}.map(detectOpenAICompletionsCompat)));\n`,
	);
	const result = spawnSync(process.execPath, [script], { encoding: "utf8", timeout: 5_000 });
	expect(result.status, `${result.stdout}\n${result.stderr}`).toBe(0);
	expect(result.stderr).toBe("");
	return JSON.parse(result.stdout) as OpenAICompletionsCompat[];
}

describe("generated OpenAI compatibility hostname matching", () => {
	it("preserves provider compatibility for canonical endpoints and explicit provider ids", () => {
		const actual = detectCompat(endpoints.map(({ hostname }) => ({ baseUrl: `https://${hostname}/v1` })));
		const explicit = detectCompat(
			endpoints.map(({ provider }) => ({ provider, baseUrl: "https://proxy.example/v1" })),
		);
		for (const [index, endpoint] of endpoints.entries()) {
			expect(actual[index], endpoint.hostname).toMatchObject(endpoint.expected);
			if (endpoint.provider) expect(actual[index], endpoint.provider).toEqual(explicit[index]);
		}
	});

	it("does not infer provider compatibility from lookalikes, userinfo, paths, queries, or fragments", () => {
		const urls = endpoints.flatMap(({ hostname }) => [
			`https://${hostname}.example/v1`,
			`https://not${hostname}/v1`,
			`https://${hostname}@example.com/v1`,
			`https://example.com/${hostname}/v1`,
			`https://example.com/v1?upstream=https://${hostname}`,
			`https://example.com/v1#${hostname}`,
		]);
		urls.push("https://api.moonshot.example/v1", "api.deepseek.com/v1", "", "not a URL");
		const [baseline, ...actual] = detectCompat([
			{ baseUrl: "https://example.com/v1" },
			...urls.map((baseUrl) => ({ baseUrl })),
		]);
		for (const [index, compat] of actual.entries()) expect(compat, urls[index]).toEqual(baseline);
	});

	it("recognizes legitimate domain subdomains and case-normalized hostnames with ports", () => {
		const cases = [
			["deepseek.com", "https://api.deepseek.com/v1"],
			["deepseek.com", "https://region.api.deepseek.com/v1"],
			["deepseek.com", "HTTPS://API.DEEPSEEK.COM:8443/v1"],
			["cerebras.ai", "https://api.cerebras.ai/v1"],
			["openrouter.ai", "https://api.openrouter.ai/v1"],
			["chutes.ai", "https://llm.chutes.ai/v1"],
			["opencode.ai", "https://api.opencode.ai/v1"],
			["api.cloudflare.com", "https://API.CLOUDFLARE.COM:443/client/v4"],
		];
		const actual = detectCompat(cases.map(([, baseUrl]) => ({ baseUrl })));
		const canonical = detectCompat(cases.map(([hostname]) => ({ baseUrl: `https://${hostname}/v1` })));
		expect(actual).toEqual(canonical);
	});
});
