import { spawnSync } from "node:child_process";
import { cpSync, mkdirSync, readFileSync, rmSync } from "node:fs";
import { dirname, join } from "node:path";
import { getPublicWorkspacePackages } from "./release-packages.mjs";

// npm pack does not bundle hoisted workspace links. Materialize the published
// files so fork releases carry their matching, unpublished upstream modules.
const packages = getPublicWorkspacePackages();
for (const pkg of packages.filter((item) => item.publishConfig?.registry === "https://npm.pkg.github.com")) {
	const manifest = JSON.parse(readFileSync(join(pkg.directory, "package.json"), "utf8"));
	for (const name of manifest.bundleDependencies ?? []) {
		const dependency = packages.find((item) => item.name === name);
		if (!dependency) throw new Error(`Missing bundled workspace ${name}`);
		// npm treats bundles as already installed. Declare their runtime dependencies
		// on the host package so npm still installs portable external dependencies.
		const dependencyManifest = JSON.parse(readFileSync(join(dependency.directory, "package.json"), "utf8"));
		for (const [external, version] of Object.entries(dependencyManifest.dependencies ?? {})) {
			if (manifest.dependencies?.[external] !== version) {
				throw new Error(`${pkg.name} must declare ${external}@${version} for bundled ${name}`);
			}
		}
		const result = spawnSync(process.platform === "win32" ? "npm.cmd" : "npm", ["pack", "--dry-run", "--ignore-scripts", "--json"], {
			cwd: dependency.directory,
			encoding: "utf8",
		});
		if (result.status !== 0) throw new Error(result.stderr || `Cannot pack ${name}`);
		const output = JSON.parse(result.stdout);
		const packed = Array.isArray(output) ? output[0] : Object.values(output)[0];
		const target = join(pkg.directory, "node_modules", name);
		rmSync(target, { recursive: true, force: true });
		for (const file of packed.files) {
			const destination = join(target, file.path);
			mkdirSync(dirname(destination), { recursive: true });
			cpSync(join(dependency.directory, file.path), destination);
		}
		console.log(`Bundled ${name}@${dependency.version} into ${pkg.name}`);
	}
}
