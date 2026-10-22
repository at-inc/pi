import { spawnSync } from "node:child_process";
import { mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";
import { afterEach, describe, expect, it, vi } from "vitest";
import { applyClassifierModelMetadata, getStaticClassifierModels } from "../scripts/classifier-model-data.ts";
import { hydrateModelCatalog } from "../scripts/hydrate-model-catalog.ts";
import { MODEL_DATA_MANIFEST_FILE, validateGeneratedModelData } from "../scripts/model-data.ts";
import type { AnyModel, ClassifierApi, ClassifierModel } from "../src/types.ts";

const roots: string[] = [];

afterEach(() => {
	vi.unstubAllGlobals();
	for (const root of roots.splice(0)) rmSync(root, { recursive: true, force: true });
});

function classifier(
	provider: string,
	id: string,
	api: ClassifierApi = "typesafe-system-one",
): ClassifierModel<ClassifierApi> {
	return {
		type: "classifier",
		id,
		name: "Jev",
		api,
		provider,
		baseUrl: "https://api.typesafe.ai/v1/",
		input: ["text"],
		cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0 },
		contextWindow: 64000,
	};
}

function catalogFixture(catalog: Record<string, unknown[]>) {
	const root = mkdtempSync(join(tmpdir(), "pi-classifier-hydration-"));
	roots.push(root);
	const providers = Object.keys(catalog);
	const providersDir = join(root, "src/providers");
	mkdirSync(providersDir, { recursive: true });
	writeFileSync(
		join(root, "src/models.generated.ts"),
		providers
			.map((provider) => {
				const prefix = provider.toUpperCase().replace(/-/g, "_");
				writeFileSync(join(providersDir, `${provider}.models.ts`), "");
				return `import { ${prefix}_CLASSIFIER_MODELS, ${prefix}_IMAGE_MODELS, ${prefix}_MODELS } from "./providers/${provider}.models.ts";`;
			})
			.join("\n"),
	);
	const path = join(root, "models.all.json");
	writeFileSync(path, JSON.stringify(catalog));
	return { root, path, providersDir, providers };
}

describe("fork classifier model metadata", () => {
	it("hydrates old classifier metadata to the same release definitions as offline generation", () => {
		const cloudflare = getStaticClassifierModels("cloudflare-workers-ai").filter(
			(model) => model.id !== "@cf/cloudflare/clef-omni" && model.id !== "typesafe/jev-1.14.0",
		);
		for (const model of cloudflare) {
			model.input = ["text"];
			delete model.compat;
			if (model.id === "@cf/cloudflare/clef-flash") {
				model.cost.input = 0.09;
				model.contextWindow = 65536;
			}
			if (model.id === "typesafe/jev") model.cost.input = 0;
		}
		const jev = classifier("typesafe", "jev-latest");
		const futureCloudflare = classifier("cloudflare-workers-ai", "future", "cloudflare-workers-ai-system-one");
		const otherApi = classifier("cloudflare-workers-ai", "custom", "custom-classifier");
		const chat = { ...cloudflare[0], type: "chat", api: "custom-chat", reasoning: false, maxTokens: 100 };
		const image = { ...cloudflare[0], type: "image", api: "custom-image", output: ["image"] };
		const otherTypeSafe = classifier("typesafe", "future");
		const proxy = classifier("proxy", "jev-latest");
		const routed = ["typesafe/jev-latest", "future/text-only"].map((id, index) => ({
			...classifier("openrouter", id),
			baseUrl: "https://openrouter.ai/api/v1",
			input: index === 0 ? ["text", "image"] : ["text"],
			cost: { input: 0.123, output: 0.456, cacheRead: 0.0123, cacheWrite: 0.00789 },
			contextWindow: 32000 + index,
		}));
		const fixture = catalogFixture({
			"cloudflare-workers-ai": [...cloudflare, futureCloudflare, otherApi, chat, image],
			typesafe: [jev, otherTypeSafe],
			proxy: [proxy],
			openrouter: routed,
		});
		const preloadPath = join(fixture.root, "mock-catalog.mjs");
		const outputPath = join(fixture.root, "generated");
		writeFileSync(
			preloadPath,
			`const decisions = ${JSON.stringify(
				routed.map((model) => ({
					id: model.id,
					name: model.name,
					context_length: model.contextWindow,
					architecture: { input_modalities: model.input, output_modalities: ["decisions"] },
					pricing: {
						prompt: String(model.cost.input / 1_000_000),
						completion: String(model.cost.output / 1_000_000),
						input_cache_read: String(model.cost.cacheRead / 1_000_000),
						input_cache_write: String(model.cost.cacheWrite / 1_000_000),
					},
				})),
			)};
globalThis.fetch = async (input) => {
  const url = String(input);
  if (url === "https://models.dev/api.json") return Response.json({});
  if (url === "https://models.dev/models.json?type=decision") return Response.json({ "typesafe/jev-latest": { name: "Jev", type: "decision", limit: { context: 64000 } } });
  if (url === "https://openrouter.ai/api/v1/models?output_modalities=decisions") return Response.json({ data: decisions });
  if (url.startsWith("https://openrouter.ai/api/v1/models") || url === "https://ai-gateway.vercel.sh/v1/models") return Response.json({ data: [] });
  if (url === "https://radius.pi.dev/v1/config") return Response.json({ baseUrl: "https://radius.pi.dev", models: [{ id: "test", name: "Test", reasoning: false, input: ["text"], cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0 }, contextWindow: 4096, maxTokens: 4096 }] });
  throw new Error(\`Unexpected fetch: \${url}\`);
};
`,
		);
		const result = spawnSync(
			process.execPath,
			[
				"--import",
				pathToFileURL(preloadPath).href,
				"scripts/generate-models.ts",
				"--json-only",
				"--json-output",
				outputPath,
			],
			{ cwd: fileURLToPath(new URL("..", import.meta.url)), encoding: "utf8", timeout: 10_000 },
		);
		expect(result.stderr).toBe("");
		expect(result.status).toBe(0);
		const generated = JSON.parse(readFileSync(join(outputPath, "models.all.json"), "utf8")) as Record<
			string,
			AnyModel[]
		>;
		const generatedCloudflare = generated["cloudflare-workers-ai"].filter((model) => model.type === "classifier");
		expect(
			generatedCloudflare.map(({ id, input, cost, contextWindow, compat }) => [
				id,
				input,
				cost,
				contextWindow,
				compat,
			]),
		).toEqual([
			[
				"@cf/cloudflare/clef",
				["text", "image"],
				{ input: 0.24, output: 0, cacheRead: 0, cacheWrite: 0 },
				65536,
				{ imageInput: "images" },
			],
			[
				"@cf/cloudflare/clef-flash",
				["text", "image"],
				{ input: 0.038, output: 0, cacheRead: 0, cacheWrite: 0 },
				24576,
				{ imageInput: "images" },
			],
			[
				"@cf/cloudflare/clef-omni",
				["text", "image"],
				{ input: 0.15, output: 0, cacheRead: 0, cacheWrite: 0 },
				64000,
				{ imageInput: "images" },
			],
			[
				"typesafe/jev",
				["text", "image"],
				{ input: 0.042, output: 0, cacheRead: 0, cacheWrite: 0 },
				32000,
				{ imageInput: "assets" },
			],
			[
				"typesafe/jev-1.14.0",
				["text", "image"],
				{ input: 0.042, output: 0, cacheRead: 0, cacheWrite: 0 },
				32000,
				{ imageInput: "assets" },
			],
		]);
		expect(generated.typesafe).toEqual([
			{ ...jev, input: ["text", "image"], compat: { imageInput: "assets", sendImageOptInHeader: true } },
		]);
		const generatedRouted = generated.openrouter.filter((model) => model.type === "classifier");
		expect(generatedRouted).toHaveLength(routed.length);
		for (const model of routed) {
			expect(generatedRouted).toContainEqual({
				...model,
				compat: { imageInput: "state-image-urls", sendImageOptInHeader: false },
			});
		}
		const fetch = vi.fn(() => {
			throw new Error("Unexpected hydration network request");
		});
		vi.stubGlobal("fetch", fetch);
		hydrateModelCatalog(fixture.root, fixture.path);
		validateGeneratedModelData(fixture.root);
		const files = [...fixture.providers.map((provider) => `${provider}.json`), MODEL_DATA_MANIFEST_FILE];
		const snapshot = files.map((file) => readFileSync(join(fixture.providersDir, "data", file), "utf8"));
		hydrateModelCatalog(fixture.root, fixture.path);
		expect(files.map((file) => readFileSync(join(fixture.providersDir, "data", file), "utf8"))).toEqual(snapshot);
		const hydrated = JSON.parse(snapshot[0]) as Record<string, Record<string, unknown>>;
		const classifiers = hydrated["cloudflare-workers-ai-system-one"];
		for (const model of generatedCloudflare) expect(classifiers[`classifier:${model.id}`]).toEqual(model);
		expect(classifiers["classifier:@cf/cloudflare/clef-flash"]).toMatchObject({ contextWindow: 24576 });
		expect(Object.keys(classifiers)).toHaveLength(6);
		expect(classifiers["classifier:future"]).toEqual(futureCloudflare);
		expect(hydrated["custom-classifier"]["classifier:custom"]).toEqual(otherApi);
		expect(hydrated["custom-chat"][`chat:${chat.id}`]).toEqual(chat);
		expect(hydrated["custom-image"][`image:${image.id}`]).toEqual(image);
		const typesafe = JSON.parse(snapshot[1]) as Record<string, Record<string, unknown>>;
		expect(typesafe["typesafe-system-one"]["classifier:jev-latest"]).toEqual(generated.typesafe[0]);
		expect(typesafe["typesafe-system-one"]["classifier:future"]).toEqual(otherTypeSafe);
		expect(JSON.parse(snapshot[2])).toEqual({ "typesafe-system-one": { "classifier:jev-latest": proxy } });
		const openrouter = JSON.parse(snapshot[3]) as Record<string, Record<string, unknown>>;
		for (const model of generatedRouted) {
			expect(openrouter["typesafe-system-one"][`classifier:${model.id}`]).toEqual(model);
		}
		expect(fetch).not.toHaveBeenCalled();
	});

	it("only normalizes compat for native OpenRouter System One classifiers", () => {
		const model = {
			...classifier("openrouter", "future/decision"),
			baseUrl: "https://custom.example.test",
			inputLimits: { images: { maxPerRequest: 3 } },
			cost: {
				input: 1,
				output: 2,
				cacheRead: 0.3,
				cacheWrite: 0.4,
				tiers: [{ inputTokensAbove: 100, input: 2, output: 4, cacheRead: 0.6, cacheWrite: 0.8 }],
			},
			compat: { imageInput: "assets", sendImageOptInHeader: true, custom: true },
			custom: { untouched: true },
		};
		const before = structuredClone(model);
		applyClassifierModelMetadata(model);
		expect(model).toEqual({
			...before,
			compat: { imageInput: "state-image-urls", sendImageOptInHeader: false, custom: true },
		});
		const unrelated = [
			{ ...before, type: "chat" },
			{ ...before, type: "image" },
			{ ...before, type: undefined },
			{ ...before, provider: "proxy" },
			{ ...before, api: "custom-classifier" },
		];
		const unchanged = structuredClone(unrelated);
		for (const entry of unrelated) applyClassifierModelMetadata(entry);
		expect(unrelated).toEqual(unchanged);
	});

	it("only replaces release-owned fields on matching classifier entries", () => {
		const model = {
			...classifier("cloudflare-workers-ai", "@cf/cloudflare/clef-flash", "cloudflare-workers-ai-system-one"),
			name: "Catalog name",
			baseUrl: "https://custom.example.test",
			contextWindow: 12345,
			compat: { imageInput: "assets", sendImageOptInHeader: false, custom: true },
			custom: { untouched: true },
		};
		const beforeCloudflare = structuredClone(model);
		applyClassifierModelMetadata(model);
		expect(model).toEqual({
			...beforeCloudflare,
			name: "Catalog name",
			baseUrl: "https://custom.example.test",
			contextWindow: 24576,
			input: ["text", "image"],
			cost: { input: 0.038, output: 0, cacheRead: 0, cacheWrite: 0 },
			compat: { imageInput: "images", sendImageOptInHeader: false, custom: true },
			custom: { untouched: true },
		});
		const jev = {
			...classifier("typesafe", "jev-latest"),
			compat: { imageInput: "images", sendImageOptInHeader: false, custom: true },
		};
		jev.cost.input = 0.123;
		const before = structuredClone(jev);
		applyClassifierModelMetadata(jev);
		expect(jev).toEqual({
			...before,
			input: ["text", "image"],
			compat: { imageInput: "assets", sendImageOptInHeader: true, custom: true },
		});
		const unrelated = [
			{ ...before, type: "chat" },
			{ ...before, type: "image" },
			{ ...before, type: undefined },
			{ ...before, provider: "proxy" },
			{ ...before, api: "custom-classifier" },
			{ ...before, id: "future" },
			{ ...beforeCloudflare, type: "chat" },
			{ ...beforeCloudflare, provider: "proxy" },
			{ ...beforeCloudflare, api: "custom-classifier" },
			{ ...beforeCloudflare, id: "future" },
		];
		const unchanged = structuredClone(unrelated);
		for (const entry of unrelated) applyClassifierModelMetadata(entry);
		expect(unrelated).toEqual(unchanged);
	});

	it("keeps backfills isolated, avoids duplicates, and never invents a TypeSafe model", () => {
		const [first] = getStaticClassifierModels("cloudflare-workers-ai");
		first.input.push("text");
		first.cost.input = 99;
		first.compat!.imageInput = "assets";
		expect(getStaticClassifierModels("cloudflare-workers-ai")[0]).toMatchObject({
			input: ["text", "image"],
			cost: { input: 0.24 },
			compat: { imageInput: "images" },
		});
		expect(getStaticClassifierModels("typesafe")).toEqual([]);
		const existing = getStaticClassifierModels("cloudflare-workers-ai");
		const fixture = catalogFixture({
			"cloudflare-workers-ai": existing,
			typesafe: [classifier("typesafe", "future")],
		});
		hydrateModelCatalog(fixture.root, fixture.path);
		const cloudflare = JSON.parse(
			readFileSync(join(fixture.providersDir, "data/cloudflare-workers-ai.json"), "utf8"),
		) as Record<string, Record<string, unknown>>;
		expect(Object.values(cloudflare["cloudflare-workers-ai-system-one"])).toEqual(existing);
		const typesafe = JSON.parse(readFileSync(join(fixture.providersDir, "data/typesafe.json"), "utf8")) as Record<
			string,
			Record<string, unknown>
		>;
		expect(Object.keys(typesafe["typesafe-system-one"])).toEqual(["classifier:future"]);
	});
});
