import { execFileSync } from "node:child_process";
import { existsSync } from "node:fs";
import { createRequire } from "node:module";
import { dirname, join } from "node:path";

export function execNpmSync(args, options = {}) {
	if (process.platform !== "win32") return execFileSync("npm", args, options);
	const npmCli = process.env.npm_execpath;
	if (!npmCli) throw new Error("Cannot locate npm on Windows. Run this command through its npm script.");
	if (!existsSync(npmCli)) throw new Error(`Cannot locate the npm CLI: ${npmCli}`);
	return execFileSync(process.execPath, [npmCli, ...args], options);
}

export function execPnpmSync(args, options = {}) {
	const packageJsonPath = createRequire(import.meta.url).resolve("pnpm");
	return execFileSync(process.execPath, [join(dirname(packageJsonPath), "bin/pnpm.mjs"), ...args], options);
}

export function parseNpmPackResult(output, packageName) {
	const parsed = JSON.parse(output);
	const entries = Array.isArray(parsed)
		? parsed
		: parsed && typeof parsed === "object" && Object.hasOwn(parsed, packageName)
			? Object.values(parsed)
			: [];
	if (
		entries.length !== 1 ||
		!entries[0] ||
		typeof entries[0].filename !== "string" ||
		!entries[0].filename ||
		entries[0].name !== packageName
	) {
		throw new Error(`npm pack returned an unexpected result for ${packageName}`);
	}
	return entries[0];
}
