#!/usr/bin/env node

import { existsSync, mkdtempSync, readFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { prepareDurablePackages } from "./prepare-durable-packages.mjs";
import { produceArtifactSet, verifyArtifactSet } from "./package-artifacts.mjs";
import { execNpmSync } from "./npm-command.mjs";

const registry = "https://npm.pkg.github.com";
const librariesOnly = process.argv.includes("--libraries-only");
const durableOnly = process.argv.includes("--durable-only");
const dryRun = process.argv.includes("--dry-run");
const unknownArgs = process.argv
	.slice(2)
	.filter((arg) => !["--dry-run", "--libraries-only", "--durable-only"].includes(arg));

if (unknownArgs.length > 0 || (librariesOnly && durableOnly)) {
	console.error("Usage: node scripts/publish-github-packages.mjs [--dry-run] [--libraries-only | --durable-only]");
	process.exit(1);
}

function readPackage(directory) {
	return JSON.parse(readFileSync(join(directory, "package.json"), "utf8"));
}

function viewPackage(name, field) {
	let output;
	try {
		output = execNpmSync(
			field === "dist-tags"
				? ["dist-tag", "ls", name, "--registry", registry]
				: ["view", name, field, "--json", "--registry", registry],
			{ encoding: "utf8", stdio: ["inherit", "pipe", "pipe"] },
		);
	} catch (error) {
		const details = [error.stdout, error.stderr].filter(Boolean).join("\n");
		if (details.includes("E404") || details.includes("404 Not Found")) return null;
		throw new Error(details || `Failed to query ${name} ${field}`, { cause: error });
	}
	if (output.trim()) {
		if (field !== "dist-tags") return JSON.parse(output);
		return Object.fromEntries(
			output
				.trim()
				.split("\n")
				.map((line) => {
					const separator = line.indexOf(": ");
					if (separator < 1) throw new Error(`Invalid dist-tag response for ${name}`);
					return [line.slice(0, separator), line.slice(separator + 2)];
				}),
		);
	}

	throw new Error(output || `Failed to query ${name} ${field}`);
}

const stagingDirectory = durableOnly ? mkdtempSync(join(tmpdir(), "pi-durable-publish-")) : undefined;
let artifactSet;
try {
	const packages = durableOnly
		? prepareDurablePackages(stagingDirectory)
		: [
				{ directory: "packages/ai", name: "@at-inc/pi-ai" },
				{ directory: "packages/agent", name: "@at-inc/pi-agent-core" },
				{ directory: "packages/coding-agent", name: "@at-inc/pi" },
			].slice(0, librariesOnly ? 2 : 3);
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

	if (durableOnly) {
		if (viewPackage(`@at-inc/pi-ai@${version}`, "version") !== version) {
			throw new Error(`Publish @at-inc/pi-ai@${version} before the Durable packages`);
		}
	}

	artifactSet = produceArtifactSet({ build: false, repoRoot: process.cwd(), packages: packageStates });
	for (const pkg of packageStates) {
		const artifact = artifactSet.getPackage(pkg.name);
		pkg.tarballPath = artifact.tarballPath;
		pkg.published = viewPackage(`${pkg.name}@${pkg.version}`, "version") !== null;
		pkg.tags = viewPackage(pkg.name, "dist-tags") ?? {};
		console.log(`${pkg.name} dist-tags before publishing: ${JSON.stringify(pkg.tags)}`);
	}

	console.log(`Publishing GitHub packages at ${version}${dryRun ? " (dry run)" : ""}\n`);

	verifyArtifactSet(artifactSet);
	let publishError;
	try {
		for (const pkg of packageStates) {
			if (pkg.published) {
				console.log(`${pkg.name}@${pkg.version} is already published; skipping.`);
				continue;
			}
			if (!dryRun) {
				verifyArtifactSet(artifactSet);
				execNpmSync(["publish", pkg.tarballPath, "--ignore-scripts", "--registry", registry, "--tag", tag], {
					cwd: pkg.directory,
					stdio: "inherit",
				});
			}
		}
	} catch (error) {
		publishError = error;
	}
	let verificationError;
	try {
		if (!dryRun && tag !== "latest") {
			for (const pkg of packageStates) {
				const tags = viewPackage(pkg.name, "dist-tags") ?? {};
				if (tags.latest !== pkg.tags.latest)
					throw new Error(`${pkg.name}: latest changed during prerelease publishing`);
				if (tags[tag] !== pkg.version) throw new Error(`${pkg.name}: ${tag} does not point to ${pkg.version}`);
				console.log(`${pkg.name}: ${tag} verified; latest unchanged (${tags.latest ?? "absent"}).`);
			}
		}
	} catch (error) {
		verificationError = error;
	}
	if (publishError && verificationError)
		throw new AggregateError([publishError, verificationError], "Publication and tag verification failed");
	if (publishError) throw publishError;
	if (verificationError) throw verificationError;
} finally {
	if (artifactSet) rmSync(artifactSet.artifactDirectory, { recursive: true, force: true });
	if (stagingDirectory) rmSync(stagingDirectory, { recursive: true, force: true });
}
