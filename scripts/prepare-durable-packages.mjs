import { execFileSync } from "node:child_process";
import { cpSync, mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";

export function prepareDurablePackages(destination) {
	const gitHead = execFileSync("git", ["rev-parse", "HEAD"], { encoding: "utf8" }).trim();
	const ai = JSON.parse(readFileSync("packages/ai/package.json", "utf8"));
	const packages = [
		{ directory: "packages/chord", sourceName: "@earendil-works/chord", name: "@at-inc/chord" },
		{ directory: "packages/durable", sourceName: "@earendil-works/pi-durable", name: "@at-inc/pi-durable" },
	];
	if (ai.name !== "@at-inc/pi-ai") throw new Error("Unexpected AI package identity");
	for (const pkg of packages) {
		const manifest = JSON.parse(readFileSync(join(pkg.directory, "package.json"), "utf8"));
		if (manifest.name !== pkg.sourceName || manifest.version !== ai.version) {
			throw new Error(`${pkg.directory} must match the fork beta's identity and version`);
		}
		const output = JSON.parse(execFileSync(process.platform === "win32" ? "npm.cmd" : "npm",
			["pack", "--dry-run", "--ignore-scripts", "--json"], { cwd: pkg.directory, encoding: "utf8" }));
		const packed = Array.isArray(output) ? output[0] : Object.values(output)[0];
		if (packed.name !== manifest.name || packed.version !== manifest.version) {
			throw new Error(`Unexpected package contents for ${pkg.sourceName}`);
		}
		const directory = resolve(destination, pkg.name.split("/")[1]);
		for (const file of packed.files) {
			const target = join(directory, file.path);
			mkdirSync(dirname(target), { recursive: true });
			cpSync(join(pkg.directory, file.path), target);
		}
		manifest.name = pkg.name;
		manifest.gitHead = gitHead;
		manifest.repository.url = "git+https://github.com/at-inc/pi.git";
		manifest.publishConfig = { registry: "https://npm.pkg.github.com" };
		if (pkg.name === "@at-inc/pi-durable") {
			manifest.dependencies["@earendil-works/chord"] = `npm:@at-inc/chord@${manifest.version}`;
			manifest.dependencies["@at-inc/pi-ai"] = manifest.version;
		}
		writeFileSync(join(directory, "package.json"), `${JSON.stringify(manifest, null, "\t")}\n`);
		pkg.directory = directory;
		console.log(`Prepared ${pkg.name}@${manifest.version} from ${gitHead}`);
	}
	return packages;
}
