// Runs the TS package's own tests (packages/agent/test) against the facade: their `../src/<module>` imports resolve to
// test/facade/<module>, which re-exports the facade module. A test file that cannot run against the facade is
// excluded below with the reason.

import { existsSync } from "node:fs";
import { dirname, join, relative, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { defineConfig, type Plugin } from "vitest/config";
import { workspaceSourcePaths } from "../../../vitest.base.ts";

const here = dirname(fileURLToPath(import.meta.url));
const tsPackage = resolve(here, "../../../packages/agent");
const tsTests = join(tsPackage, "test");
const facadeShims = join(here, "test/facade");
const tsSource = (path: string) => resolve(here, "../../../packages", path);

/** Test files that cannot run against the facade, with the reason. */
export const skipped: Record<string, string> = {};

function facadeModules(): Plugin {
	return {
		name: "pi-facade-modules",
		enforce: "pre",
		resolveId(source, importer) {
			if (!importer?.startsWith(tsTests) || !source.startsWith("../src/")) return null;
			const shim = join(facadeShims, source.slice("../src/".length));
			if (!existsSync(shim)) {
				throw new Error(`${relative(tsPackage, importer)} imports ${source}, which the facade does not provide`);
			}
			return shim;
		},
	};
}

export default defineConfig({
	plugins: [facadeModules()],
	resolve: {
		conditions: ["source"],
		// The TS packages the tests import, from source, as in packages/agent/vitest.config.ts.
		alias: [
			{ find: /^@earendil-works\/chord$/, replacement: workspaceSourcePaths.chordIndex },
			{ find: /^@earendil-works\/chord\/context$/, replacement: workspaceSourcePaths.chordContext },
			{ find: /^@earendil-works\/chord\/delta$/, replacement: workspaceSourcePaths.chordDelta },
			{ find: /^@earendil-works\/pi-telemetry$/, replacement: tsSource("telemetry/src/index.ts") },
			{ find: /^@at-inc\/pi-ai$/, replacement: tsSource("ai/src/index.ts") },
			{ find: /^@at-inc\/pi-ai\/compat$/, replacement: tsSource("ai/src/compat.ts") },
			{ find: /^@at-inc\/pi-ai\/providers\/faux$/, replacement: tsSource("ai/src/providers/faux.ts") },
			{ find: /^@at-inc\/pi-ai\/utils\/uuid$/, replacement: tsSource("ai/src/utils/uuid.ts") },
			{ find: /^@at-inc\/pi-ai-rs$/, replacement: resolve(here, "../pi-ai/src/index.ts") },
		],
	},
	ssr: { resolve: { conditions: ["source"] } },
	test: {
		root: tsPackage,
		include: ["test/*.test.ts", relative(tsPackage, join(here, "test/*.test.ts"))],
		exclude: Object.keys(skipped),
		testTimeout: 30000,
	},
});
