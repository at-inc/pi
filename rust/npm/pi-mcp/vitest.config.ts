// Runs the TS package's own tests against the facade: their `../src/<module>` imports resolve to test/facade/<module>,
// which re-exports the facade module. A test that imports a module the facade does not provide fails to resolve, so
// the tests that need one are excluded below with the reason.

import { existsSync } from "node:fs";
import { dirname, join, relative, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { defineConfig, type Plugin } from "vitest/config";

const here = dirname(fileURLToPath(import.meta.url));
const tsPackage = resolve(here, "../../../packages/mcp");
const tsTests = join(tsPackage, "test");
const facadeShims = join(here, "test/facade");

/** Tests that cannot run against the facade, with the reason. */
export const skipped: Record<string, string> = {};

function facadeModules(): Plugin {
	return {
		name: "pi-facade-modules",
		enforce: "pre",
		resolveId(source, importer) {
			if (!importer?.startsWith(tsTests) || !source.startsWith("../src/")) return null;
			const shim = join(facadeShims, source.slice("../src/".length));
			if (!existsSync(shim)) {
				throw new Error(`${relative(tsPackage, importer)} imports ${source}, which this facade does not provide`);
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
		include: ["test/*.test.ts"],
		exclude: Object.keys(skipped),
		testTimeout: 30000,
	},
});
