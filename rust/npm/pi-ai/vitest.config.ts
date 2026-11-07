// Runs the TS package's own openai-completions tests against the facade: their `../src/<module>` imports resolve to
// test/facade/<module>, which re-exports the facade module (and bridges a mocked `openai` SDK, see
// test/facade/openai-bridge.ts). Only the modules this slice provides are mapped; a test that imports another module
// fails to resolve, so the tests that need one are excluded below with the reason.

import { existsSync } from "node:fs";
import { dirname, join, relative, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { defineConfig, type Plugin } from "vitest/config";

const here = dirname(fileURLToPath(import.meta.url));
const tsPackage = resolve(here, "../../../packages/ai");
const tsTests = join(tsPackage, "test");
const facadeShims = join(here, "test/facade");

/** Tests of the slice's modules that cannot run against the facade, with the reason. */
export const skipped: Record<string, string> = {
	"test/azure-openai-completions.test.ts":
		"imports ../src/providers/azure.ts; providers belong to the pi-ai models slice",
	"test/anthropic-auth-token.test.ts":
		"imports ../src/env-api-keys.ts, ../src/providers/anthropic.ts and ../src/utils/identity.ts and uses createModels({ authContext }); needs provider factories and authContext, bridged by the models slice",
	"test/anthropic-eager-tool-input-e2e.test.ts":
		"e2e (skips without credentials); imports ../src/env-api-keys.ts and test/oauth.ts, which imports ../src/providers/all.ts; needs provider factories, bridged by the models slice",
	"test/anthropic-empty-thinking-signature-compat.test.ts": "needs compat getModels, bridged by the models slice",
	"test/anthropic-federation.test.ts":
		"needs providers/anthropic and createModels({ authContext }), bridged by the models slice; its stream tests also assert the SDK's federation config, which the wire cannot reproduce (the core exchanges the identity token itself)",
	"test/anthropic-long-cache-retention-e2e.test.ts":
		"e2e (skips without credentials); imports ../src/env-api-keys.ts and test/oauth.ts, which imports ../src/providers/all.ts; needs provider factories, bridged by the models slice",
	"test/anthropic-sdk-migration.test.ts":
		"imports ../src/utils/identity.ts (formatUserAgent), not bridged yet; the core's User-Agent on wasm is 'pi/<version> (browser)', where the TS on Node reports its platform and runtime",
	"test/anthropic-tool-name-normalization.test.ts":
		"e2e (skips without an OAuth token); test/oauth.ts imports ../src/providers/all.ts; needs provider factories, bridged by the models slice",
	"test/pi-messages.test.ts":
		"its registration test needs compat getApiProvider (the api registry), bridged by the models slice",
	"test/github-copilot-anthropic.test.ts": "needs models getSupportedThinkingLevels, bridged by the models slice",
	"test/fireworks-model-generation.test.ts":
		"needs models getSupportedThinkingLevels and hasApi, bridged by the models slice",
	"test/fork-model-fallbacks.test.ts": "needs models getSupportedThinkingLevels, bridged by the models slice",
};

function facadeModules(): Plugin {
	return {
		name: "pi-facade-modules",
		enforce: "pre",
		resolveId(source, importer) {
			if (!importer?.startsWith(tsTests) || !source.startsWith("../src/")) return null;
			const shim = join(facadeShims, source.slice("../src/".length));
			if (!existsSync(shim)) {
				throw new Error(
					`${relative(tsPackage, importer)} imports ${source}, which this facade slice does not provide`,
				);
			}
			return shim;
		},
	};
}

export default defineConfig({
	plugins: [facadeModules()],
	resolve: { conditions: ["source"] },
	ssr: { resolve: { conditions: ["source"] } },
	test: {
		root: tsPackage,
		include: [
			"test/openai-completions-*.test.ts",
			"test/azure-openai-completions.test.ts",
			"test/anthropic-auth-token.test.ts",
			"test/anthropic-cache-write-1h-cost.test.ts",
			"test/anthropic-eager-tool-input-compat.test.ts",
			"test/anthropic-eager-tool-input-e2e.test.ts",
			"test/anthropic-empty-thinking-signature-compat.test.ts",
			"test/anthropic-error-tool-result-images.test.ts",
			"test/anthropic-federation.test.ts",
			"test/anthropic-federation-sdk.test.ts",
			"test/anthropic-force-adaptive-thinking.test.ts",
			"test/anthropic-long-cache-retention-e2e.test.ts",
			"test/anthropic-mid-conversation-effort.test.ts",
			"test/anthropic-opus-4-8-smoke.test.ts",
			"test/anthropic-sdk-migration.test.ts",
			"test/anthropic-sse-parsing.test.ts",
			"test/anthropic-strict-tool-fallback.test.ts",
			"test/anthropic-strict-tool-schema.test.ts",
			"test/anthropic-temperature-compat.test.ts",
			"test/anthropic-thinking-binding-e2e.test.ts",
			"test/anthropic-thinking-disable.test.ts",
			"test/anthropic-tool-name-normalization.test.ts",
			"test/pi-messages.test.ts",
			"test/github-copilot-anthropic.test.ts",
			"test/transform-messages-*.test.ts",
			"test/lax-message-content.test.ts",
			"test/fireworks-model-generation.test.ts",
			"test/fork-model-fallbacks.test.ts",
		],
		exclude: Object.keys(skipped),
		testTimeout: 30000,
	},
});
