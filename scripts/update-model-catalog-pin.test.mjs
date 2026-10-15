import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { afterEach, beforeEach, mock, test } from "node:test";
import YAML from "yaml";
import { hydrateModelCatalogBytes, hydratePinnedModelCatalog, updateModelCatalogPin } from "./update-model-catalog-pin.mjs";

const model = {
	type: "chat",
	id: "model-a",
	name: "Model A",
	api: "openai-completions",
	provider: "test-provider",
	baseUrl: "https://example.test/v1",
	reasoning: false,
	input: ["text"],
	cost: { input: 1, output: 2, cacheRead: 0, cacheWrite: 0 },
	contextWindow: 1000,
	maxTokens: 100,
};
const catalog = (models) => `${JSON.stringify({ "test-provider": models })}\n`;
const revisionOf = (body) => `sha256-${createHash("sha256").update(body).digest("hex")}`;
const pinFile = (revision) => `${JSON.stringify({ revision }, null, 2)}\n`;
const pinned = async (promise) => {
	const { bytes, ...result } = await promise;
	assert.equal(bytes.toString("utf8"), bodies.get(result.revision));
	return result;
};

const liveBody = catalog([model, { ...model, id: "model-b" }]);
const liveRevision = revisionOf(liveBody);
const currentBody = catalog([model]);
const currentRevision = revisionOf(currentBody);
// Hydration fails when a checkout provider is missing, e.g. after main adds one.
const staleBody = `${JSON.stringify({ "other-provider": [model] })}\n`;
const staleRevision = revisionOf(staleBody);
// Hydrates, but lacks a model type the live catalog (generated from main) has.
const classifier = { ...model, type: "classifier", id: "classifier-a", name: "Classifier A" };
delete classifier.reasoning;
delete classifier.maxTokens;
const classifierBody = catalog([model, classifier]);
const classifierRevision = revisionOf(classifierBody);
// Hydrates with the same model types, but main switched the provider's API (#10985).
const switchedBody = catalog([{ ...model, api: "anthropic-messages" }]);
const switchedRevision = revisionOf(switchedBody);
const bodies = new Map([
	[liveRevision, liveBody],
	[currentRevision, currentBody],
	[staleRevision, staleBody],
	[classifierRevision, classifierBody],
	[switchedRevision, switchedBody],
]);
const typed = "types=chat,image,classifier";

test("keeps ordinary CI and scoped beta builds on the committed catalog", () => {
	for (const [workflow, job] of [["ci.yml", "build-check-test"], ["env.yml", "check"]]) {
		const config = YAML.parse(readFileSync(new URL(`../.github/workflows/${workflow}`, import.meta.url), "utf8"));
		const steps = config.jobs[job].steps;
		const hydrate = steps.findIndex((step) => step.run === "node scripts/update-model-catalog-pin.mjs --hydrate");
		const build = steps.findIndex((step) => step.run === "npm run build:offline");
		assert.ok(hydrate >= 0 && build > hydrate, workflow);
		assert.ok(!steps.some((step) => step.run === "npm run build"), workflow);
	}
	const publish = YAML.parse(
		readFileSync(new URL("../.github/workflows/publish-github-packages.yml", import.meta.url), "utf8"),
	).jobs.publish.steps;
	const hydrate = publish.find((step) => step.run === "node scripts/update-model-catalog-pin.mjs --hydrate");
	assert.match(hydrate.if, /inputs\.libraries_only/);
	assert.match(hydrate.if, /inputs\.durable_only/);
	assert.match(publish.find((step) => step.name === "Build").run, /build:offline/);
});

let root;
beforeEach(() => {
	root = mkdtempSync(join(tmpdir(), "pi-catalog-pin-"));
	mkdirSync(join(root, "packages/ai/src/providers"), { recursive: true });
	mkdirSync(join(root, "nix"));
	writeFileSync(
		join(root, "packages/ai/src/models.generated.ts"),
		'import { TEST_PROVIDER_CLASSIFIER_MODELS, TEST_PROVIDER_IMAGE_MODELS, TEST_PROVIDER_MODELS } from "./providers/test-provider.models.ts";\n',
	);
	writeFileSync(join(root, "packages/ai/src/providers/test-provider.models.ts"), "");
	writeFileSync(join(root, "nix/model-catalog.json"), pinFile(currentRevision));
});
afterEach(() => {
	mock.restoreAll();
	rmSync(root, { recursive: true, force: true });
});

function readPin() {
	return readFileSync(join(root, "nix/model-catalog.json"), "utf8");
}

function mockPiDev({ live = liveRevision, failure } = {}) {
	const requests = [];
	mock.method(globalThis, "fetch", async (url) => {
		requests.push(url);
		const revision = /\/revisions\/(sha256-[0-9a-f]+)\?/.exec(url)?.[1];
		if (revision) {
			if (failure === "revision-http") return new Response(null, { status: 503 });
			return new Response(failure === "revision-hash" ? "wrong" : bodies.get(revision));
		}
		if (failure === "discovery-http") return new Response(null, { status: 503 });
		return new Response(failure === "discovery-hash" ? "wrong" : bodies.get(live), {
			headers: { "x-pi-model-catalog-revision": failure === "discovery-revision" ? "latest" : live },
		});
	});
	return requests;
}

test("hydrates only the committed revision without changing the pin or generated model types", async () => {
	const requests = mockPiDev();
	const generatedPath = join(root, "packages/ai/src/models.generated.ts");
	const generated = readFileSync(generatedPath, "utf8");
	assert.equal(await hydratePinnedModelCatalog(root), currentRevision);
	assert.deepEqual(requests, [`https://pi.dev/api/models/revisions/${currentRevision}?${typed}`]);
	assert.equal(readPin(), pinFile(currentRevision));
	assert.equal(readFileSync(generatedPath, "utf8"), generated);
	const data = JSON.parse(readFileSync(join(root, "packages/ai/src/providers/data/test-provider.json"), "utf8"));
	assert.deepEqual(Object.keys(data["openai-completions"]), ["chat:model-a"]);
});

test("rejects invalid hydration pins before making a request", async () => {
	const requests = mockPiDev();
	writeFileSync(join(root, "nix/model-catalog.json"), pinFile("latest"));
	await assert.rejects(hydratePinnedModelCatalog(root), /Invalid pin/);
	assert.deepEqual(requests, []);
});

for (const failure of ["revision-http", "revision-hash"]) {
	test(`preserves hydrated data and the pin on ${failure} failure`, async () => {
		hydrateModelCatalogBytes(root, Buffer.from(currentBody));
		const path = join(root, "packages/ai/src/providers/data/test-provider.json");
		const before = readFileSync(path, "utf8");
		mockPiDev({ failure });
		await assert.rejects(hydratePinnedModelCatalog(root));
		assert.equal(readFileSync(path, "utf8"), before);
		assert.equal(readPin(), pinFile(currentRevision));
	});
}

test("preserves hydrated data when the pinned catalog cannot hydrate this checkout", async () => {
	hydrateModelCatalogBytes(root, Buffer.from(currentBody));
	const path = join(root, "packages/ai/src/providers/data/test-provider.json");
	const before = readFileSync(path, "utf8");
	writeFileSync(join(root, "nix/model-catalog.json"), pinFile(staleRevision));
	mockPiDev();
	await assert.rejects(hydratePinnedModelCatalog(root), /missing provider/);
	assert.equal(readFileSync(path, "utf8"), before);
	assert.equal(readPin(), pinFile(staleRevision));
});

test("pins the newest typed catalog after verifying its immutable URL", async () => {
	const requests = mockPiDev();
	assert.deepEqual(await pinned(updateModelCatalogPin(root)), { revision: liveRevision, updated: true });
	// No pi-version: main's version is the last release, which pi.dev answers
	// with the catalog for released clients (#10985).
	assert.deepEqual(requests, [
		`https://pi.dev/api/models?${typed}`,
		`https://pi.dev/api/models/revisions/${liveRevision}?${typed}`,
	]);
	assert.equal(readPin(), pinFile(liveRevision));
});

test("keeps a pin that still hydrates the checkout and has the live model groups", async () => {
	const requests = mockPiDev();
	assert.deepEqual(await pinned(updateModelCatalogPin(root, { ifStale: true })), {
		revision: currentRevision,
		updated: false,
	});
	assert.deepEqual(requests, [
		`https://pi.dev/api/models/revisions/${currentRevision}?${typed}`,
		`https://pi.dev/api/models?${typed}`,
		`https://pi.dev/api/models/revisions/${liveRevision}?${typed}`,
	]);
	assert.equal(readPin(), pinFile(currentRevision));
});

test("replaces a pin that lacks a model type of the live catalog", async (t) => {
	const errors = [];
	t.mock.method(console, "error", (message) => errors.push(message));
	mockPiDev({ live: classifierRevision });
	assert.deepEqual(await pinned(updateModelCatalogPin(root, { ifStale: true })), {
		revision: classifierRevision,
		updated: true,
	});
	assert.equal(readPin(), pinFile(classifierRevision));
	assert.deepEqual(errors, [
		"Pinned model catalog is stale: model groups differ (missing test-provider/classifier/openai-completions)",
	]);
});

// #10985
test("replaces a pin whose provider uses a different API than the live catalog", async (t) => {
	const errors = [];
	t.mock.method(console, "error", (message) => errors.push(message));
	writeFileSync(join(root, "nix/model-catalog.json"), pinFile(switchedRevision));
	mockPiDev();
	assert.deepEqual(await pinned(updateModelCatalogPin(root, { ifStale: true })), { revision: liveRevision, updated: true });
	assert.equal(readPin(), pinFile(liveRevision));
	assert.deepEqual(errors, [
		"Pinned model catalog is stale: model groups differ (missing test-provider/chat/openai-completions; extra test-provider/chat/anthropic-messages)",
	]);
});

test("replaces a pin that no longer hydrates the checkout", async (t) => {
	t.mock.method(console, "error", () => {});
	writeFileSync(join(root, "nix/model-catalog.json"), pinFile(staleRevision));
	mockPiDev();
	assert.deepEqual(await pinned(updateModelCatalogPin(root, { ifStale: true })), { revision: liveRevision, updated: true });
	assert.equal(readPin(), pinFile(liveRevision));
});

test("refuses to pin a live catalog that cannot hydrate the checkout", async () => {
	mockPiDev({ live: staleRevision });
	await assert.rejects(updateModelCatalogPin(root), /cannot hydrate this checkout/);
	assert.equal(readPin(), pinFile(currentRevision));
});

for (const failure of ["discovery-http", "discovery-revision", "discovery-hash", "revision-http", "revision-hash"]) {
	test(`keeps the old pin on ${failure} failure`, async () => {
		mockPiDev({ failure });
		await assert.rejects(updateModelCatalogPin(root));
		assert.equal(readPin(), pinFile(currentRevision));
	});
}

test("does not treat an unreachable pinned revision as stale", async () => {
	mockPiDev({ failure: "revision-http" });
	await assert.rejects(updateModelCatalogPin(root, { ifStale: true }), /unavailable/);
	assert.equal(readPin(), pinFile(currentRevision));
});
