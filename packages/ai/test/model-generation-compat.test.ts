import { describe, expect, it } from "vitest";
import { buildCatalog } from "../catalog/build.ts";
import type { ProviderDefinition } from "../catalog/define.ts";
import { PROVIDERS } from "../catalog/providers/index.ts";
import type { CatalogSources } from "../catalog/sources/index.ts";
import type { Model, OpenAICompletionsCompat } from "../src/types.ts";

const sources: CatalogSources = {
	modelsDev: {},
	modelsDevDecisions: {},
	openRouter: { chat: [], images: [], classifiers: [] },
	aiGateway: { chat: [], classifiers: [] },
	nvidiaModelIds: new Map(),
	radius: [],
};

const endpoints = [
	"api.z.ai",
	"open.bigmodel.cn",
	"api.together.ai",
	"api.together.xyz",
	"api.moonshot.ai",
	"api.moonshot.cn",
	"openrouter.ai",
	"api.cloudflare.com",
	"gateway.ai.cloudflare.com",
	"integrate.api.nvidia.com",
	"api.ant-ling.com",
	"cerebras.ai",
	"deepseek.com",
	"api.x.ai",
	"chutes.ai",
	"opencode.ai",
];

function catalogModel(baseUrl: string, provider: string): Model<"openai-completions"> {
	return {
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
	};
}

function generateCompat(baseUrl: string, definition: ProviderDefinition): OpenAICompletionsCompat | undefined {
	const provider = {
		...definition,
		models: () => [catalogModel(baseUrl, definition.id)],
		images: undefined,
		classifiers: undefined,
	};
	return buildCatalog([provider], sources, false)[provider.id].chat[0].compat as OpenAICompletionsCompat | undefined;
}

describe("generated OpenAI compatibility provider selection", () => {
	it("preserves explicit provider compatibility independently of endpoint URLs", () => {
		const cases: [string, OpenAICompletionsCompat][] = [
			["zai", { thinkingFormat: "zai", supportsReasoningEffort: false }],
			["zai-coding-cn", { thinkingFormat: "zai", supportsReasoningEffort: false }],
			["together", { thinkingFormat: "together", supportsLongCacheRetention: false }],
			["moonshotai", { maxTokensField: "max_tokens", supportsReasoningEffort: false }],
			["moonshotai-cn", { maxTokensField: "max_tokens", supportsReasoningEffort: false }],
			["openrouter", { thinkingFormat: "openrouter", sendSessionAffinityHeaders: true }],
			["cloudflare-workers-ai", { supportsStore: false, supportsLongCacheRetention: false }],
			["cloudflare-ai-gateway", { maxTokensField: "max_tokens", supportsLongCacheRetention: false }],
			["nvidia", { maxTokensField: "max_tokens", supportsLongCacheRetention: false }],
			["ant-ling", { thinkingFormat: "ant-ling", supportsLongCacheRetention: false }],
			["cerebras", { supportsStore: false, supportsDeveloperRole: false }],
			["deepseek", { thinkingFormat: "deepseek", requiresReasoningContentOnAssistantMessages: true }],
			["opencode", { supportsStore: false, supportsDeveloperRole: false }],
		];
		for (const [id, expected] of cases) {
			const definition = PROVIDERS.find((provider) => provider.id === id);
			if (!definition) throw new Error(`Missing provider ${id}`);
			const baseline = generateCompat("https://proxy.example/v1", definition);
			expect(baseline, id).toMatchObject(expected);
			for (const hostname of endpoints) {
				expect(generateCompat(`https://${hostname}/v1`, definition), id).toEqual(baseline);
			}
		}
	});

	it("does not infer provider compatibility from lookalikes, userinfo, paths, queries, or fragments", () => {
		const urls = endpoints.flatMap((hostname) => [
			`https://${hostname}.example/v1`,
			`https://not${hostname}/v1`,
			`https://${hostname}@example.com/v1`,
			`https://example.com/${hostname}/v1`,
			`https://example.com/v1?upstream=https://${hostname}`,
			`https://example.com/v1#${hostname}`,
		]);
		urls.push("https://api.moonshot.example/v1", "api.deepseek.com/v1", "", "not a URL");
		for (const baseUrl of urls) {
			expect(generateCompat(baseUrl, { id: "custom" }), baseUrl).toBeUndefined();
		}
	});

	it("does not infer provider compatibility even from canonical domains or legitimate subdomains", () => {
		for (const hostname of endpoints) {
			for (const baseUrl of [
				`https://${hostname}/v1`,
				`https://region.${hostname}/v1`,
				`HTTPS://${hostname.toUpperCase()}:8443/v1`,
			]) {
				expect(generateCompat(baseUrl, { id: "custom" }), baseUrl).toBeUndefined();
			}
		}
	});

	it("preserves explicit model compatibility over provider defaults", () => {
		const definition = PROVIDERS.find((provider) => provider.id === "nvidia");
		if (!definition) throw new Error("Missing provider nvidia");
		const catalog = buildCatalog(
			[
				{
					...definition,
					models: () => [
						{
							...catalogModel("https://proxy.example/v1", "nvidia"),
							compat: { supportsStore: true, maxTokensField: "max_completion_tokens" },
						},
					],
				},
			],
			sources,
			false,
		);
		expect(catalog.nvidia.chat[0].compat).toMatchObject({
			supportsStore: true,
			maxTokensField: "max_completion_tokens",
			supportsLongCacheRetention: false,
		});
	});
});
