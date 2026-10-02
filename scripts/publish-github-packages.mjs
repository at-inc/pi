#!/usr/bin/env node

import { existsSync, readFileSync } from "node:fs";
import { spawnSync } from "node:child_process";
import { join } from "node:path";

const registry = "https://npm.pkg.github.com";
const librariesOnly = process.argv.includes("--libraries-only");
const packages = [
	{ directory: "packages/ai", name: "@at-inc/pi-ai" },
	{ directory: "packages/agent", name: "@at-inc/pi-agent-core" },
	{ directory: "packages/coding-agent", name: "@at-inc/pi" },
].slice(0, librariesOnly ? 2 : 3);
const dryRun = process.argv.includes("--dry-run");
const unknownArgs = process.argv.slice(2).filter((arg) => !["--dry-run", "--libraries-only"].includes(arg));

if (unknownArgs.length > 0) {
	console.error("Usage: node scripts/publish-github-packages.mjs [--dry-run] [--libraries-only]");
	process.exit(1);
}

function commandForPlatform(command) {
	return process.platform === "win32" ? `${command}.cmd` : command;
}

function run(command, args, options = {}) {
	console.log(`$ ${[command, ...args].join(" ")}`);
	const result = spawnSync(commandForPlatform(command), args, {
		cwd: options.cwd,
		encoding: "utf8",
		stdio: options.capture ? ["inherit", "pipe", "pipe"] : "inherit",
	});

	if (result.status !== 0) {
		const output = [result.stdout, result.stderr].filter(Boolean).join("\n");
		throw new Error(output || `Command failed: ${command} ${args.join(" ")}`);
	}

	return result;
}

function readPackage(directory) {
	return JSON.parse(readFileSync(join(directory, "package.json"), "utf8"));
}

function viewPackage(name, field) {
	const result = spawnSync(
		commandForPlatform("npm"),
		["view", name, field, "--json", "--registry", registry],
		{ encoding: "utf8", stdio: ["inherit", "pipe", "pipe"] },
	);

	if (result.status === 0 && result.stdout.trim()) return JSON.parse(result.stdout);

	const output = [result.stdout, result.stderr].filter(Boolean).join("\n");
	if (result.status !== 0 && (output.includes("E404") || output.includes("404 Not Found"))) return null;
	throw new Error(output || `Failed to query ${name} ${field}`);
}

const packageStates = packages.map((pkg) => {
	const manifest = readPackage(pkg.directory);
	if (manifest.name !== pkg.name) {
		throw new Error(`${pkg.directory}/package.json has name ${manifest.name}, expected ${pkg.name}`);
	}
	if (!manifest.publishConfig || manifest.publishConfig.registry !== registry) {
		throw new Error(`${pkg.directory}/package.json must publish to ${registry}`);
	}
	if (!existsSync(join(pkg.directory, "dist"))) {
		throw new Error(`${pkg.directory}/dist does not exist. Run npm run build before publishing.`);
	}
	return { ...pkg, version: manifest.version, manifest };
});

const versions = new Set(packageStates.map((pkg) => pkg.version));
if (versions.size !== 1) {
	throw new Error(`GitHub packages are not lockstep versioned: ${[...versions].join(", ")}`);
}

const version = packageStates[0].version;
const match = /^(?:0|[1-9]\d*)\.(?:0|[1-9]\d*)\.(?:0|[1-9]\d*)(?:-(beta|rev)\.(?:0|[1-9]\d*))?$/.exec(version);
if (!match) throw new Error(`Unsupported version or prerelease channel: ${version}`);
const tag = match[1] ?? "latest";

run("node", ["scripts/prepare-github-package-bundles.mjs"]);

for (const pkg of packageStates) {
	const result = run("npm", ["pack", "--dry-run", "--ignore-scripts", "--json"], { cwd: pkg.directory, capture: true });
	const output = JSON.parse(result.stdout);
	const packed = Array.isArray(output) ? output[0] : Object.values(output)[0];
	const files = new Set(packed.files.map((file) => file.path));
	const required = ["package.json", pkg.manifest.main, pkg.manifest.types, ...Object.values(pkg.manifest.bin ?? {}),
		...(pkg.manifest.bundleDependencies ?? []).map((name) => `node_modules/${name}/package.json`)];
	if (packed.name !== pkg.name || packed.version !== pkg.version ||
		required.filter(Boolean).some((file) => !files.has(file.replace(/^\.\//, "")))) {
		throw new Error(`${pkg.name} package contents are incomplete or do not match the manifest`);
	}
	pkg.published = viewPackage(`${pkg.name}@${pkg.version}`, "version") !== null;
	pkg.tags = viewPackage(pkg.name, "dist-tags") ?? {};
	console.log(`${pkg.name} dist-tags before publishing: ${JSON.stringify(pkg.tags)}`);
}

console.log(`Publishing GitHub packages at ${packageStates[0].version}${dryRun ? " (dry run)" : ""}\n`);

try {
	for (const pkg of packageStates) {
		if (pkg.published) {
			console.log(`${pkg.name}@${pkg.version} is already published; skipping.`);
			continue;
		}
		if (!dryRun) {
			run("npm", ["publish", "--ignore-scripts", "--registry", registry, "--tag", tag], { cwd: pkg.directory });
		}
	}
} finally {
	if (!dryRun && tag !== "latest") {
		for (const pkg of packageStates) {
			const tags = viewPackage(pkg.name, "dist-tags") ?? {};
			if (tags.latest !== pkg.tags.latest) throw new Error(`${pkg.name}: latest changed during prerelease publishing`);
			if (tags[tag] !== pkg.version) throw new Error(`${pkg.name}: ${tag} does not point to ${pkg.version}`);
			console.log(`${pkg.name}: ${tag} verified; latest unchanged (${tags.latest ?? "absent"}).`);
		}
	}
}
