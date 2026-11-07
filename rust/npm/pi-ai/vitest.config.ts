// Runs the TS package's own openai-completions and auth/OAuth tests against the facade: their `../src/<module>` imports
// resolve to test/facade/<module>, which re-exports the facade module (and bridges a mocked `openai` SDK, see
// test/facade/openai-bridge.ts). Only the modules the slices provide are mapped; a test that imports another module
// fails to resolve, so the tests that need one are excluded below with the reason. test/auth holds the facade's own
// tests for behavior whose TS test cannot run yet.

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
		"imports ../src/api/anthropic-messages.ts and ../src/providers/anthropic.ts; both belong to the pi-ai models slice",
	"test/github-copilot-oauth.test.ts":
		"imports ../src/providers/github-copilot.ts; providers belong to the pi-ai models slice",
	"test/kimi-coding-oauth.test.ts":
		"imports ../src/providers/kimi-coding.ts; providers belong to the pi-ai models slice",
	"test/oauth-auth.test.ts":
		"imports ../src/providers/{anthropic,github-copilot,openai}.ts; providers belong to the pi-ai models slice",
	"test/openrouter-oauth.test.ts":
		"imports ../src/providers/openrouter.ts; providers belong to the pi-ai models slice",
	"test/models-runtime.test.ts":
		"registers plain-object providers (Models.setProvider) and calls calculateCost, both of the pi-ai models slice; login and logout are covered by test/auth/models-login.test.ts",
	"test/models-entry.test.ts":
		"spawns node on the TS package's own src and dist (packages/ai), so it never loads the facade",
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
			"test/*oauth*.test.ts",
			"test/anthropic-auth-token.test.ts",
			"test/models-runtime.test.ts",
			"test/models-entry.test.ts",
			join(here, "test/auth/*.test.ts"),
		],
		exclude: Object.keys(skipped),
		testTimeout: 30000,
	},
});
