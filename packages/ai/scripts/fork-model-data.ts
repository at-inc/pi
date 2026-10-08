import type { AnyModel, ImageModel } from "../src/types.ts";

const OPENAI_FAST_MODE_MODEL_IDS = new Set([
	"gpt-6-astra",
	"gpt-6-sol",
	"gpt-6.1-sol",
	"gpt-6-luna",
	"gpt-5.6-sol",
	"gpt-5.6-terra",
	"gpt-5.6-luna",
	"gpt-5.5",
	"gpt-5.4",
]);
const ANTHROPIC_FAST_MODE_MODEL_IDS = new Set([
	"claude-opus-5-5",
	"claude-sonnet-5",
	"claude-opus-4-8",
	"claude-opus-4-6",
]);
const CHAT_METADATA_APIS = new Set([
	"openai-completions",
	"openai-responses",
	"azure-openai-responses",
	"openai-codex-responses",
	"anthropic-messages",
	"bedrock-converse-stream",
	"mistral-conversations",
	"google-generative-ai",
	"google-vertex",
	"pi-messages",
]);
const FORK_AZURE_CONTEXT_WINDOW_OVERRIDES = new Map([
	["gpt-6-astra", 1050000],
	["gpt-6-luna", 1050000],
	["gpt-6-sol", 1050000],
	["gpt-6.1-sol", 1050000],
]);
const DEFAULT_IMAGE_RESIZE = {
	maxWidth: 2000,
	maxHeight: 2000,
	maxBytes: 4.5 * 1024 * 1024,
	jpegQuality: 80,
} as const;

export interface ForkModelData {
	id: string;
	api: string;
	type?: unknown;
	provider?: unknown;
	thinkingLevelMap?: unknown;
	supportsFastMode?: unknown;
	contextWindow?: unknown;
}

export function applyForkModelMetadata(model: ForkModelData): void {
	if ((model.type !== undefined && model.type !== "chat") || !CHAT_METADATA_APIS.has(model.api)) return;
	if (
		((model.provider === "openai" || model.provider === "cloudflare-ai-gateway") &&
			model.api === "openai-responses" &&
			OPENAI_FAST_MODE_MODEL_IDS.has(model.id)) ||
		(model.provider === "openai-codex" &&
			model.api === "openai-codex-responses" &&
			OPENAI_FAST_MODE_MODEL_IDS.has(model.id)) ||
		(model.provider === "anthropic" &&
			model.api === "anthropic-messages" &&
			ANTHROPIC_FAST_MODE_MODEL_IDS.has(model.id))
	) {
		model.supportsFastMode = true;
	}

	const responseApi =
		model.api === "openai-responses" ||
		model.api === "azure-openai-responses" ||
		model.api === "openai-codex-responses";
	const thinkingLevelMap: Record<string, string | null> = {};
	if (model.id.includes("opus-5-5") || model.id.includes("opus.5.5")) {
		Object.assign(thinkingLevelMap, { off: null, minimal: null });
	}
	if (model.id === "gpt-6-astra" && responseApi) {
		Object.assign(thinkingLevelMap, {
			off: null,
			minimal: null,
			low: "low",
			medium: "medium",
			high: "high",
			xhigh: "xhigh",
			max: "max",
		});
	} else if ((model.id.includes("gpt-6-astra") || model.id.includes("gpt-6.1-sol")) && !responseApi) {
		Object.assign(thinkingLevelMap, { off: null, minimal: null });
	}
	if ((model.id.includes("gpt-6-sol") || model.id.includes("gpt-6-luna")) && model.provider !== "openai-codex") {
		thinkingLevelMap.minimal = null;
	}
	if (Object.keys(thinkingLevelMap).length > 0) {
		const existing = model.thinkingLevelMap;
		model.thinkingLevelMap = {
			...(typeof existing === "object" && existing !== null && !Array.isArray(existing) ? existing : {}),
			...thinkingLevelMap,
		};
	}
	if (model.provider === "azure" && model.api === "azure-openai-responses") {
		const contextWindow = FORK_AZURE_CONTEXT_WINDOW_OVERRIDES.get(model.id);
		if (contextWindow !== undefined) model.contextWindow = contextWindow;
	}
}

export function applyImageInputMetadata(model: AnyModel): void {
	if (!model.input.includes("image")) return;

	const providerLimits: AnyModel["inputLimits"] =
		model.provider === "anthropic"
			? {
					maxRequestBytes: 32 * 1024 * 1024,
					images: { maxPerRequest: model.type !== "image" && model.contextWindow === 200000 ? 100 : 600 },
				}
			: model.provider === "amazon-bedrock"
				? { images: { maxPerMessage: 20 } }
				: model.provider === "openai"
					? { maxRequestBytes: 512 * 1024 * 1024, images: { maxPerRequest: 1500 } }
					: model.provider === "google"
						? { maxRequestBytes: 20 * 1024 * 1024, images: { maxPerRequest: 3600 } }
						: undefined;
	const configuredImages = model.inputLimits?.images;
	model.inputLimits = {
		...providerLimits,
		...model.inputLimits,
		images: {
			...providerLimits?.images,
			...configuredImages,
			resize: { ...DEFAULT_IMAGE_RESIZE, ...configuredImages?.resize },
		},
	};
}

export function getForkImageModels(provider: string): ImageModel<"openai-codex-images">[] {
	if (provider !== "openai-codex") return [];
	const model: ImageModel<"openai-codex-images"> = {
		type: "image",
		id: "chatgpt-image-generation",
		name: "ChatGPT Image Generation",
		api: "openai-codex-images",
		provider: "openai-codex",
		baseUrl: "https://chatgpt.com/backend-api",
		input: ["text", "image"],
		output: ["image"],
		cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0 },
	};
	applyImageInputMetadata(model);
	return [model];
}
