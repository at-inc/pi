// Builds the facade package in the current directory: its JavaScript with tsc, and for every module its exports map
// names a declaration file that re-exports the TS package's own, so the facade's public types are the TS package's.
// Usage: node ../scripts/build-facade.mjs <specifier of the TS package> <directory of the TS package>

import { execFileSync } from "node:child_process";
import { readdirSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { basename, join } from "node:path";

const [specifier, tsDir] = process.argv.slice(2);
if (!specifier || !tsDir) throw new Error("usage: build-facade.mjs <TS package specifier> <TS package directory>");

const facadeExports = JSON.parse(readFileSync("package.json", "utf-8")).exports;
const tsExports = JSON.parse(readFileSync(join(tsDir, "package.json"), "utf-8")).exports;

rmSync("dist", { recursive: true, force: true });
// Types are checked by `npm run check`; the build only emits JavaScript, since the declarations come from the TS package.
execFileSync("tsc", ["-p", "tsconfig.build.json", "--noCheck", "--declaration", "false", "--declarationMap", "false"], {
	stdio: "inherit",
});

const missing = [];
for (const [key, target] of Object.entries(facadeExports)) {
	if (typeof target !== "object" || !target.types) continue;
	const tsTypes = tsExports[key]?.types;
	if (!tsTypes) throw new Error(`${key} is not in the exports map of ${specifier}`);
	const subpath = (name) => specifier + key.slice(1).replaceAll("*", name);
	if (!key.includes("*")) {
		writeFileSync(target.types, `export * from "${subpath("")}";\n`);
		continue;
	}
	const [prefix, suffix] = target.types.split("*");
	const emitted = new Set(
		readdirSync(prefix.replace(/\/$/, ""))
			.filter((file) => file.endsWith(".js"))
			.map((file) => basename(file, ".js")),
	);
	const [tsPrefix, tsSuffix] = tsTypes.split("*");
	for (const file of readdirSync(join(tsDir, tsPrefix))) {
		if (!file.endsWith(tsSuffix)) continue;
		const name = file.slice(0, -tsSuffix.length);
		if (emitted.has(name)) writeFileSync(prefix + name + suffix, `export * from "${subpath(name)}";\n`);
		else missing.push(subpath(name));
	}
}
if (missing.length > 0) console.warn(`Modules of ${specifier} without a facade module: ${missing.join(", ")}`);
