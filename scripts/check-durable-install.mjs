#!/usr/bin/env node

import assert from "node:assert/strict";
import { execFileSync, spawnSync } from "node:child_process";
import { copyFileSync, existsSync, mkdirSync, mkdtempSync, readFileSync, realpathSync, rmSync, writeFileSync } from "node:fs";
import { findPackageJSON } from "node:module";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { packReleasePackages } from "./coding-agent-consumer.mjs";
import { prepareDurablePackages } from "./prepare-durable-packages.mjs";

const fromRegistry = process.argv.includes("--registry");
if (process.argv.slice(2).some((argument) => argument !== "--registry")) {
	throw new Error("Usage: node scripts/check-durable-install.mjs [--registry]");
}
const repo = process.cwd();
const version = JSON.parse(readFileSync("packages/ai/package.json", "utf8")).version;
const gitHead = execFileSync("git", ["rev-parse", "HEAD"], { encoding: "utf8" }).trim();
const packages = [
	{ name: "@at-inc/pi-ai", alias: "@earendil-works/pi-ai", directory: "packages/ai" },
	{ name: "@at-inc/pi-agent-core", alias: "@earendil-works/pi-agent-core", directory: "packages/agent" },
	{ name: "@at-inc/chord", alias: "@earendil-works/chord" },
	{ name: "@at-inc/pi-durable", alias: "@earendil-works/pi-durable" },
];
const root = mkdtempSync(join(tmpdir(), "pi-durable-install-"));

function run(command, args, options = {}) {
	console.log(`$ ${[command, ...args].join(" ")}`);
	const result = spawnSync(process.platform === "win32" && ["npm", "pnpm"].includes(command) ? `${command}.cmd` : command, args, {
		encoding: "utf8", stdio: "inherit", timeout: 300_000, ...options,
	});
	if (result.error) throw result.error;
	if (result.status !== 0) throw new Error(`${command} exited with status ${result.status}`);
}

try {
	let tarballs;
	if (!fromRegistry) {
		run(process.execPath, ["scripts/prepare-github-package-bundles.mjs"]);
		const staged = prepareDurablePackages(join(root, "staged"));
		tarballs = packReleasePackages([...packages.slice(0, 2), ...staged], join(root, "tarballs"));
	}
	const directory = join(root, "consumer");
	mkdirSync(directory);
	writeFileSync(join(directory, ".npmrc"), "@at-inc:registry=https://npm.pkg.github.com\n");
	writeFileSync(join(directory, "package.json"), `${JSON.stringify({
		private: true,
		type: "module",
		dependencies: Object.fromEntries(packages.map((pkg) => [pkg.alias,
			fromRegistry ? `npm:${pkg.name}@${version}` : `file:${tarballs.get(pkg.name)}`])),
	}, null, "\t")}\n`);
	writeFileSync(join(directory, "pnpm-workspace.yaml"), JSON.stringify(fromRegistry ? {} : {
		overrides: {
			"@at-inc/pi-ai": `file:${tarballs.get("@at-inc/pi-ai")}`,
			"@earendil-works/chord": `file:${tarballs.get("@at-inc/chord")}`,
		},
	}));
	run("pnpm", ["install", "--ignore-scripts", "--no-frozen-lockfile"], { cwd: directory });
	const consumerManifest = join(directory, "package.json");
	const imports = [];
	for (const pkg of packages) {
		const installedDirectory = realpathSync(join(directory, "node_modules", pkg.alias));
		const manifest = JSON.parse(readFileSync(join(installedDirectory, "package.json"), "utf8"));
		assert.equal(manifest.name, pkg.name);
		assert.equal(manifest.version, version);
		const installedManifest = join(installedDirectory, "package.json");
		if (pkg.name !== "@at-inc/pi-ai" && pkg.name !== "@at-inc/chord") {
			assert.equal(realpathSync(findPackageJSON("@at-inc/pi-ai", installedManifest)), realpathSync(findPackageJSON("@earendil-works/pi-ai", consumerManifest)));
		}
		if (pkg.name === "@at-inc/pi-durable") {
			assert.equal(realpathSync(findPackageJSON("@earendil-works/chord", installedManifest)), realpathSync(findPackageJSON("@earendil-works/chord", consumerManifest)));
			assert.equal(manifest.dependencies["@earendil-works/chord"], `npm:@at-inc/chord@${version}`);
			assert.equal(manifest.dependencies["@at-inc/pi-ai"], version);
		}
		if (pkg.name === "@at-inc/chord" || pkg.name === "@at-inc/pi-durable") {
			assert.match(manifest.gitHead, /^[0-9a-f]{40}$/);
			if (spawnSync("git", ["cat-file", "-e", `${manifest.gitHead}^{commit}`], { stdio: "ignore" }).status !== 0) {
				run("git", ["fetch", "--no-tags", "--depth=1", "origin", manifest.gitHead]);
			}
			run("git", ["diff", "--exit-code", manifest.gitHead, gitHead, "--", "packages/chord", "packages/durable",
				"tsconfig.base.json", "package.json", "package-lock.json"]);
			for (const [subpath, conditions] of Object.entries(manifest.exports)) {
				if (typeof conditions === "string") {
					assert.ok(existsSync(join(installedDirectory, conditions)));
					continue;
				}
				for (const condition of ["types", "import"]) assert.ok(existsSync(join(installedDirectory, conditions[condition])));
				imports.push(`import * as entry${imports.length} from ${JSON.stringify(pkg.alias + subpath.slice(1))};`);
			}
			console.log(`Verified ${pkg.name}@${version}: gitHead ${manifest.gitHead}; exports ${Object.keys(manifest.exports).join(", ")}`);
		}
	}
	writeFileSync(join(directory, "exports.mjs"), `${imports.join("\n")}\nconsole.log("All Durable and Chord runtime entrypoints loaded.");\n`);
	copyFileSync(new URL("./fixtures/durable-consumer.ts", import.meta.url), join(directory, "smoke.ts"));
	writeFileSync(join(directory, "tsconfig.json"), `${JSON.stringify({
		compilerOptions: {
			target: "ES2023", module: "NodeNext", moduleResolution: "NodeNext", strict: true, skipLibCheck: true,
			noEmit: true, types: ["node"], typeRoots: [resolve(repo, "node_modules/@types")],
		},
		include: ["smoke.ts"],
	}, null, "\t")}\n`);
	run(process.execPath, [join(repo, "node_modules/typescript/bin/tsc"), "--project", join(directory, "tsconfig.json")]);
	const env = { PATH: process.env.PATH, HOME: root, USERPROFILE: root, PI_OFFLINE: "1", PI_TELEMETRY: "0", AWS_EC2_METADATA_DISABLED: "true" };
	for (const key of ["SystemRoot", "SYSTEMROOT", "WINDIR", "COMSPEC", "PATHEXT"]) {
		if (process.env[key]) env[key] = process.env[key];
	}
	run(process.execPath, ["exports.mjs"], { cwd: directory, env, timeout: 30_000 });
	for (const runtime of [process.execPath, "bun"]) run(runtime, ["smoke.ts"], { cwd: directory, env, timeout: 45_000 });
	console.log(`Verified fork Durable and Chord ${version} from ${fromRegistry ? "GitHub Packages" : "local tarballs"} with single AI and Chord runtimes.`);
} finally {
	rmSync(root, { recursive: true, force: true });
}
