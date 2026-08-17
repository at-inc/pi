#!/usr/bin/env node

import { existsSync, readFileSync } from "node:fs";
import { spawnSync } from "node:child_process";
import { join } from "node:path";

const registry = "https://npm.pkg.github.com";
const packages = [
	{ directory: "packages/agent", name: "@at-inc/pi-agent-core" },
	{ directory: "packages/coding-agent", name: "@at-inc/pi" },
];
const dryRun = process.argv.includes("--dry-run");
const unknownArgs = process.argv.slice(2).filter((arg) => arg !== "--dry-run");

if (unknownArgs.length > 0) {
	console.error("Usage: node scripts/publish-github-packages.mjs [--dry-run]");
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

function isPublished(name, version) {
	const result = spawnSync(
		commandForPlatform("npm"),
		["view", `${name}@${version}`, "version", "--json", "--registry", registry],
		{ encoding: "utf8", stdio: ["inherit", "pipe", "pipe"] },
	);

	if (result.status === 0 && result.stdout.trim()) return true;

	const output = [result.stdout, result.stderr].filter(Boolean).join("\n");
	if (result.status !== 0 && (output.includes("E404") || output.includes("404 Not Found"))) return false;
	throw new Error(output || `Failed to query ${name}@${version}`);
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
	return { ...pkg, version: manifest.version, published: isPublished(pkg.name, manifest.version) };
});

const versions = new Set(packageStates.map((pkg) => pkg.version));
if (versions.size !== 1) {
	throw new Error(`GitHub packages are not lockstep versioned: ${[...versions].join(", ")}`);
}

console.log(`Publishing GitHub packages at ${packageStates[0].version}${dryRun ? " (dry run)" : ""}\n`);

for (const pkg of packageStates) {
	if (pkg.published) {
		console.log(`${pkg.name}@${pkg.version} is already published; skipping.`);
		continue;
	}

	run("npm", ["pack", "--dry-run", "--ignore-scripts"], { cwd: pkg.directory });
	if (!dryRun) {
		run("npm", ["publish", "--ignore-scripts", "--registry", registry], { cwd: pkg.directory });
	}
}
