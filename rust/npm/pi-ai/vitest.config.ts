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
		include: ["test/openai-completions-*.test.ts", "test/azure-openai-completions.test.ts"],
		exclude: Object.keys(skipped),
		testTimeout: 30000,
	},
});
