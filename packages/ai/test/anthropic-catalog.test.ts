import { describe, expect, it } from "vitest";
import { buildCatalog } from "../catalog/build.ts";
import anthropic from "../catalog/providers/anthropic.ts";
import githubCopilot from "../catalog/providers/github-copilot.ts";
import type { CatalogSources } from "../catalog/sources/index.ts";
import type { ModelsDevV2Model } from "../catalog/sources/models-dev.ts";

const retainedIds = ["claude-fable-5-2", "claude-haiku-5-5", "claude-opus-5-5", "claude-sonnet-5-5"];
const removedIds = [
	"claude-3-5-sonnet-20241022",
	"claude-3-haiku-20240307",
	"claude-sonnet-4-6",
	"claude-sonnet-5",
	"claude-fable-5",
	"claude-fable-5-1",
	"claude-opus-4-5",
	"claude-opus-4-6",
	"claude-opus-4-8",
	"claude-opus-5",
	"claude-haiku-4-5",
];

function sourcesFor(ids: string[]): CatalogSources {
	const models = Object.fromEntries(
		ids.map((id): [string, ModelsDevV2Model] => [
			id,
			{
				id,
				type: "chat",
				name: `Upstream ${id}`,
				modalities: { input: ["text", "image"], output: ["text"] },
				capabilities: { tools: { supported: true }, reasoning: { supported: true } },
				api: { messages: { base_url: "https://example.invalid" } },
			},
		]),
	);
	return {
		modelsDev: { anthropic: { models }, "github-copilot": { models } },
		modelsDevDecisions: {},
		openRouter: { chat: [], images: [], classifiers: [] },
		aiGateway: { chat: [], classifiers: [] },
		nvidiaModelIds: new Map(),
		radius: [],
	};
}

describe("Anthropic catalog selection", () => {
	it("keeps only the four selected versions without filtering other providers", () => {
		const sources = sourcesFor([...retainedIds, ...removedIds]);
		const catalog = buildCatalog([anthropic, githubCopilot], sources, false);
		expect(catalog.anthropic.chat.map((model) => model.id).sort()).toEqual(retainedIds);
		for (const id of retainedIds) {
			expect(catalog.anthropic.chat.find((model) => model.id === id)?.name).toBe(`Upstream ${id}`);
		}
		expect(catalog["github-copilot"].chat.some((model) => model.id === "claude-opus-4-6")).toBe(true);
	});

	it("retains authored Fable and Haiku entries when upstream omits them", () => {
		const catalog = buildCatalog(
			[anthropic],
			sourcesFor(["claude-opus-5-5", "claude-sonnet-5-5", ...removedIds]),
			false,
		);
		expect(catalog.anthropic.chat.map((model) => model.id).sort()).toEqual(retainedIds);
		expect(catalog.anthropic.chat.find((model) => model.id === "claude-fable-5-2")?.name).toBe("Claude Fable 5.2");
		expect(catalog.anthropic.chat.find((model) => model.id === "claude-haiku-5-5")?.name).toBe("Claude Haiku 5.5");
	});

	it.each([
		[200000, 100],
		[1000000, 600],
	])("derives image request limits from a %i-token context window", (contextWindow, maxPerRequest) => {
		const sources = sourcesFor(["claude-haiku-5-5"]);
		sources.modelsDev.anthropic.models!["claude-haiku-5-5"].limit = { context: contextWindow, output: 128000 };
		const catalog = buildCatalog([anthropic], sources, false);
		expect(catalog.anthropic.chat.find((model) => model.id === "claude-haiku-5-5")?.inputLimits).toMatchObject({
			maxRequestBytes: 32 * 1024 * 1024,
			images: { maxPerRequest },
		});
	});
});
