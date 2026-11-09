import { buildCatalog } from "../catalog/build.ts";
import fireworks from "../catalog/providers/fireworks.ts";
import type { ModelsDevV2Model } from "../catalog/sources/models-dev.ts";
import type { Model } from "../src/types.ts";

export function createFireworksToggleModel(): Model<"anthropic-messages"> {
	const source: ModelsDevV2Model = {
		id: "accounts/fireworks/models/nemotron-3-ultra-nvfp4",
		name: "Fireworks toggle-only fixture",
		type: "chat",
		modalities: { input: ["text"], output: ["text"] },
		capabilities: { tools: { supported: true }, reasoning: { supported: true, toggle: true } },
		limit: { context: 262144, output: 32768 },
		api: { messages: { base_url: "https://api.fireworks.ai/inference" } },
	};
	const catalog = buildCatalog(
		[fireworks],
		{
			modelsDev: { "fireworks-ai": { models: { [source.id]: source } } },
			modelsDevDecisions: {},
			openRouter: { chat: [], images: [], classifiers: [] },
			aiGateway: { chat: [], classifiers: [] },
			nvidiaModelIds: new Map(),
			radius: [],
		},
		true,
	);
	const [model] = catalog.fireworks.chat;
	if (model.api !== "anthropic-messages") throw new Error("Expected Fireworks Messages API");
	return model as Model<"anthropic-messages">;
}
