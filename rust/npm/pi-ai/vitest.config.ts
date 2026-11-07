// Runs the TS package's own openai-completions, utils and auth/OAuth tests against the facade: their `../src/<module>`
// imports resolve to test/facade/<module>, which re-exports the facade module (and bridges a mocked `openai` SDK, see
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
	"test/kimi-coding-endpoints.test.ts":
		"imports ../src/auth/oauth/kimi-coding.ts; OAuth belongs to the pi-ai auth slice",
	"test/unicode-surrogate.test.ts": "e2e against real providers (credentials), through APIs of the pi-ai models slice",
	"test/context-overflow.test.ts": "e2e against real providers (credentials), through APIs of the pi-ai models slice",
	"test/abort.test.ts": "e2e against real providers (credentials), through APIs of the pi-ai models slice",
	"test/tokens.test.ts": "e2e against real providers (credentials), through APIs of the pi-ai models slice",
	"test/total-tokens.test.ts": "e2e against real providers (credentials), through APIs of the pi-ai models slice",
};

const loneSurrogate =
	"a Rust string cannot hold an unpaired surrogate: one at the end of the text read so far is held back, others read as U+FFFD";
const quadraticSnapshots =
	"each changed snapshot crosses the boundary whole (JSON text), so a long stream costs time and memory quadratic in its length";

/** Tests of included files that cannot pass against the facade, by test name, with the reason. */
export const skippedTests: Record<string, string> = {
	"allocates linearly in the streamed text, never per accumulated text": quadraticSnapshots,
	"keeps very large open arrays linear by letting their preview lag": quadraticSnapshots,
	"matches the reference after every piece, at every chunk size, with insignificant whitespace": loneSurrogate,
	"matches today's parseStreamingJson at every prefix of compact documents": loneSurrogate,
	"joins surrogate pairs split across pieces, escaped or raw": loneSurrogate,
	"unpaired high surrogate, minLength 2": loneSurrogate,
	"reads every JSONTestSuite document JSON.parse accepts exactly like it, and never throws on the rest":
		"1e400 reads as null: a serde_json::Value cannot hold Infinity",
	"merges request identity over the Models and wrapper identities":
		"Models passes no custom fetch to a JS API implementation yet (options.rs stream_options_to_js, models slice)",
	"sends the OpenCode session header to custom providers pointing at OpenCode":
		"Models passes no custom fetch to a JS API implementation yet (options.rs stream_options_to_js, models slice)",
};

const escapeRegExp = (text: string) => text.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");

/** The TS tests of the utility modules (utils slice). */
const utilsTests = [
	"context-estimate",
	"error-body",
	"event-stream",
	"identity",
	"json-parse",
	"kimi-coding-endpoints",
	"node-http-proxy",
	"oauth-page",
	"openrouter-cost",
	"overflow",
	"provider-retry",
	"retry",
	"streaming-json",
	"system-message-replay",
	"text",
	"transcript-tool-changes",
	"uuid",
	"validation",
	"validation-probes",
	"unicode-surrogate",
	"context-overflow",
	"abort",
	"tokens",
	"total-tokens",
].map((name) => `test/${name}.test.ts`);

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
			...utilsTests,
			join(here, "test/auth/*.test.ts"),
		],
		exclude: Object.keys(skipped),
		testNamePattern: new RegExp(`^(?!.*(?:${Object.keys(skippedTests).map(escapeRegExp).join("|")})$)`),
		testTimeout: 30000,
	},
});
