#!/usr/bin/env node

import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { copyFileSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { packReleasePackages } from "./coding-agent-consumer.mjs";

const fromRegistry = process.argv.includes("--registry");
if (process.argv.slice(2).some((argument) => argument !== "--registry")) {
	throw new Error("Usage: node scripts/check-library-install.mjs [--registry]");
}
const packages = [
	{ directory: "packages/ai", name: "@at-inc/pi-ai", alias: "@earendil-works/pi-ai" },
	{ directory: "packages/agent", name: "@at-inc/pi-agent-core", alias: "@earendil-works/pi-agent-core" },
];
const manifests = packages.map((pkg) => JSON.parse(readFileSync(join(pkg.directory, "package.json"), "utf8")));
const version = manifests[0].version;
assert.equal(manifests[1].version, version);
const repo = process.cwd();
const root = mkdtempSync(join(tmpdir(), "pi-library-install-"));

function run(command, args, options = {}) {
	console.log(`$ ${[command, ...args].join(" ")}`);
	const result = spawnSync(process.platform === "win32" && command === "npm" ? "npm.cmd" : command, args, {
		encoding: "utf8",
		stdio: "inherit",
		timeout: 300_000,
		...options,
	});
	if (result.error) throw result.error;
	if (result.status !== 0) throw new Error(`${command} exited with status ${result.status}`);
}

try {
	let tarballs;
	if (!fromRegistry) {
		run(process.execPath, ["scripts/prepare-github-package-bundles.mjs"]);
		tarballs = packReleasePackages(packages, join(root, "tarballs"));
	}
	const directory = join(root, "consumer");
	mkdirSync(directory);
	writeFileSync(join(directory, ".npmrc"), "@at-inc:registry=https://npm.pkg.github.com\n");
	writeFileSync(join(directory, "package.json"), `${JSON.stringify({
		private: true,
		type: "module",
		dependencies: Object.fromEntries(packages.map((pkg) => [
			pkg.alias,
			fromRegistry ? `npm:${pkg.name}@${version}` : `file:${tarballs.get(pkg.name)}`,
		])),
		...(fromRegistry ? {} : { overrides: { "@at-inc/pi-ai": `file:${tarballs.get("@at-inc/pi-ai")}` } }),
	}, null, "\t")}\n`);
	run("npm", ["install", "--ignore-scripts", "--no-audit", "--no-fund"], { cwd: directory });
	for (const pkg of packages) {
		const installed = JSON.parse(readFileSync(join(directory, "node_modules", pkg.alias, "package.json"), "utf8"));
		assert.equal(installed.name, pkg.name);
		assert.equal(installed.version, version);
	}
	copyFileSync(new URL("./fixtures/library-consumer.ts", import.meta.url), join(directory, "smoke.ts"));
	writeFileSync(join(directory, "tsconfig.json"), `${JSON.stringify({
		compilerOptions: {
			target: "ES2023",
			module: "NodeNext",
			moduleResolution: "NodeNext",
			strict: true,
			skipLibCheck: true,
			noEmit: true,
			types: ["node"],
			typeRoots: [resolve(repo, "node_modules/@types")],
		},
		include: ["smoke.ts"],
	}, null, "\t")}\n`);
	run(process.execPath, [join(repo, "node_modules/typescript/bin/tsc"), "--project", join(directory, "tsconfig.json")]);
	const env = { PATH: process.env.PATH, HOME: root, USERPROFILE: root, PI_OFFLINE: "1", PI_TELEMETRY: "0", AWS_EC2_METADATA_DISABLED: "true" };
	for (const key of ["SystemRoot", "SYSTEMROOT", "WINDIR", "COMSPEC", "PATHEXT"]) {
		if (process.env[key]) env[key] = process.env[key];
	}
	for (const runtime of [process.execPath, "bun"]) {
		run(runtime, ["smoke.ts"], { cwd: directory, env, timeout: 30_000 });
	}
	console.log(`Verified both libraries at ${version} from ${fromRegistry ? "GitHub Packages" : "local tarballs"} using bro-components aliases.`);
} finally {
	rmSync(root, { recursive: true, force: true });
}
