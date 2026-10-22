import { CLOUDFLARE_WORKERS_AI_REST_BASE_URL } from "../src/api/cloudflare.ts";
import type { ClassifierModel } from "../src/types.ts";

// Workers AI has no unauthenticated catalog, so its System One models are listed here from
// Cloudflare's model pages.
const CLOUDFLARE_WORKERS_AI_CLASSIFIER_MODELS: ClassifierModel<"cloudflare-workers-ai-system-one">[] = [
	// Cloudflare-hosted Clef decision models. They take up to four PNG, JPEG, or WebP images as data URLs.
	// Clef Omni also takes audio and video, which classifier contexts do not carry.
	// Pricing and context: https://developers.cloudflare.com/workers-ai/models/clef/,
	// https://developers.cloudflare.com/workers-ai/models/clef-flash/,
	// and https://developers.cloudflare.com/workers-ai/models/clef-omni/
	{
		type: "classifier",
		id: "@cf/cloudflare/clef",
		name: "Clef",
		api: "cloudflare-workers-ai-system-one",
		provider: "cloudflare-workers-ai",
		baseUrl: CLOUDFLARE_WORKERS_AI_REST_BASE_URL,
		input: ["text", "image"],
		cost: { input: 0.24, output: 0, cacheRead: 0, cacheWrite: 0 },
		contextWindow: 65536,
		compat: { imageInput: "images" },
	},
	{
		type: "classifier",
		id: "@cf/cloudflare/clef-flash",
		name: "Clef Flash",
		api: "cloudflare-workers-ai-system-one",
		provider: "cloudflare-workers-ai",
		baseUrl: CLOUDFLARE_WORKERS_AI_REST_BASE_URL,
		input: ["text", "image"],
		cost: { input: 0.038, output: 0, cacheRead: 0, cacheWrite: 0 },
		contextWindow: 24576,
		compat: { imageInput: "images" },
	},
	{
		type: "classifier",
		id: "@cf/cloudflare/clef-omni",
		name: "Clef Omni",
		api: "cloudflare-workers-ai-system-one",
		provider: "cloudflare-workers-ai",
		baseUrl: CLOUDFLARE_WORKERS_AI_REST_BASE_URL,
		input: ["text", "image"],
		cost: { input: 0.15, output: 0, cacheRead: 0, cacheWrite: 0 },
		contextWindow: 64000,
		compat: { imageInput: "images" },
	},
	// TypeSafe's Jev models take PNG and JPEG images as System One assets.
	// https://developers.cloudflare.com/ai/models/typesafe/jev/ (jev-latest)
	// and https://developers.cloudflare.com/ai/models/typesafe/jev-1.14.0/
	{
		type: "classifier",
		id: "typesafe/jev",
		name: "Jev",
		api: "cloudflare-workers-ai-system-one",
		provider: "cloudflare-workers-ai",
		baseUrl: CLOUDFLARE_WORKERS_AI_REST_BASE_URL,
		input: ["text", "image"],
		cost: { input: 0.042, output: 0, cacheRead: 0, cacheWrite: 0 },
		contextWindow: 32000,
		compat: { imageInput: "assets" },
	},
	{
		type: "classifier",
		id: "typesafe/jev-1.14.0",
		name: "Jev 1.14",
		api: "cloudflare-workers-ai-system-one",
		provider: "cloudflare-workers-ai",
		baseUrl: CLOUDFLARE_WORKERS_AI_REST_BASE_URL,
		input: ["text", "image"],
		cost: { input: 0.042, output: 0, cacheRead: 0, cacheWrite: 0 },
		contextWindow: 32000,
		compat: { imageInput: "assets" },
	},
];

export function getStaticClassifierModels(provider: string): ClassifierModel<"cloudflare-workers-ai-system-one">[] {
	return provider === "cloudflare-workers-ai" ? structuredClone(CLOUDFLARE_WORKERS_AI_CLASSIFIER_MODELS) : [];
}

export function applyClassifierModelMetadata(model: {
	type?: unknown;
	id: string;
	api: string;
	provider?: unknown;
	input?: unknown;
	compat?: unknown;
	cost?: unknown;
	contextWindow?: unknown;
}): void {
	if (model.type !== "classifier") return;
	const cloudflare =
		model.provider === "cloudflare-workers-ai" && model.api === "cloudflare-workers-ai-system-one"
			? CLOUDFLARE_WORKERS_AI_CLASSIFIER_MODELS.find((candidate) => candidate.id === model.id)
			: undefined;
	// models.dev lists text input only, but the API accepts image assets as an experimental
	// feature behind an opt-in header, which the typesafe-system-one API sends.
	// https://github.com/typesafe-ai/typesafe-sdk-js (v0.7.0 changelog)
	const typesafe = model.provider === "typesafe" && model.api === "typesafe-system-one" && model.id === "jev-latest";
	// OpenRouter answers TypeSafe `assets` with 200 but drops the images.
	const openrouter = model.provider === "openrouter" && model.api === "typesafe-system-one";
	if (cloudflare === undefined && !typesafe && !openrouter) return;
	if (!openrouter) model.input = cloudflare === undefined ? ["text", "image"] : [...cloudflare.input];
	const compat = model.compat;
	model.compat = {
		...(typeof compat === "object" && compat !== null && !Array.isArray(compat) ? compat : {}),
		...(openrouter
			? { imageInput: "state-image-urls", sendImageOptInHeader: false }
			: (cloudflare?.compat ?? { imageInput: "assets", sendImageOptInHeader: true })),
	};
	if (cloudflare !== undefined) {
		model.cost = { ...cloudflare.cost };
		model.contextWindow = cloudflare.contextWindow;
	}
}
