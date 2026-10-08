#!/usr/bin/env node

import { execFileSync } from "node:child_process";
import { copyFileSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { parseArgs } from "node:util";
import {
	installConsumer,
	verifyPackageFiles,
	verifyPnpmConsumerArtifacts,
	verifySingleRuntime,
} from "./local-package-install.mjs";
import { produceArtifactSet, readArtifactSet } from "./package-artifacts.mjs";
import { execPnpmSync } from "./npm-command.mjs";

const { values } = parseArgs({
	options: { registry: { type: "boolean", default: false }, manifest: { type: "string" } },
});
if (values.registry && values.manifest) throw new Error("Use either --registry or --manifest, not both");
const packages = [
	{ directory: "packages/ai", name: "@at-inc/pi-ai", alias: "@earendil-works/pi-ai" },
	{ directory: "packages/agent", name: "@at-inc/pi-agent-core", alias: "@earendil-works/pi-agent-core" },
];
const repo = process.cwd();
const root = mkdtempSync(join(tmpdir(), "pi-library-install-"));
const directory = join(root, "consumer");
const packageAliases = Object.fromEntries(packages.map((pkg) => [pkg.alias, pkg.name]));
const packageNames = packages.map((pkg) => pkg.alias);
const devDependencies = Object.fromEntries(
	["typescript", "@types/node"].map((name) => [
		name,
		JSON.parse(readFileSync(join(repo, "node_modules", name, "package.json"), "utf8")).version,
	]),
);

try {
	let artifactSet;
	if (!values.registry) {
		if (values.manifest) artifactSet = readArtifactSet(values.manifest);
		else {
			artifactSet = produceArtifactSet({ repoRoot: repo, build: false, outDir: join(root, "artifacts"), packages });
		}
		if (artifactSet.source === null) throw new Error("Library verification requires artifact source provenance");
	}
	const versions = packages.map((pkg) =>
		artifactSet
			? artifactSet.getPackage(pkg.name).version
			: JSON.parse(readFileSync(join(repo, pkg.directory, "package.json"), "utf8")).version,
	);
	if (new Set(versions).size !== 1) throw new Error("Library packages must have matching versions");
	const version = versions[0];
	mkdirSync(directory);
	writeFileSync(join(directory, ".npmrc"), "@at-inc:registry=https://npm.pkg.github.com\n");
	if (artifactSet)
		installConsumer({
			artifactSet,
			directory,
			packageNames,
			packageAliases,
			packageManager: "pnpm",
			devDependencies,
		});
	else {
		writeFileSync(
			join(directory, "package.json"),
			`${JSON.stringify({
				private: true,
				type: "module",
				devDependencies,
				dependencies: Object.fromEntries(packages.map((pkg) => [pkg.alias, `npm:${pkg.name}@${version}`])),
			})}\n`,
		);
		writeFileSync(join(directory, "pnpm-workspace.yaml"), "{}\n");
		execPnpmSync(["install", "--ignore-scripts", "--no-frozen-lockfile"], {
			cwd: directory,
			stdio: "inherit",
			timeout: 300_000,
		});
	}
	for (const pkg of packages)
		verifyPackageFiles({ directory, packageName: pkg.alias, expectedName: pkg.name, version });
	if (artifactSet) verifyPnpmConsumerArtifacts({ directory, artifactSet, packageNames, packageAliases });
	verifySingleRuntime({ directory, packageNames, runtimeNames: ["@earendil-works/pi-ai"] });
	copyFileSync(new URL("./fixtures/library-consumer.ts", import.meta.url), join(directory, "smoke.ts"));
	writeFileSync(
		join(directory, "tsconfig.json"),
		`${JSON.stringify({
			compilerOptions: {
				target: "ES2023",
				module: "NodeNext",
				moduleResolution: "NodeNext",
				strict: true,
				skipLibCheck: true,
				noEmit: true,
				types: ["node"],
			},
			include: ["smoke.ts"],
		})}\n`,
	);
	execFileSync(
		process.execPath,
		[join(directory, "node_modules/typescript/bin/tsc"), "--project", join(directory, "tsconfig.json")],
		{
			cwd: directory,
			stdio: "inherit",
			timeout: 300_000,
		},
	);
	const env = {
		PATH: process.env.PATH,
		HOME: root,
		USERPROFILE: root,
		PI_OFFLINE: "1",
		PI_TELEMETRY: "0",
		AWS_EC2_METADATA_DISABLED: "true",
	};
	for (const key of ["SystemRoot", "SYSTEMROOT", "WINDIR", "COMSPEC", "PATHEXT"]) {
		if (process.env[key]) env[key] = process.env[key];
	}
	for (const runtime of [process.execPath, "bun"])
		execFileSync(runtime, ["smoke.ts"], { cwd: directory, env, stdio: "inherit", timeout: 30_000 });
	console.log(
		`Verified both libraries at ${version} from ${values.registry ? "GitHub Packages" : resolve(artifactSet.manifestPath)} using bro-components aliases and one AI runtime.`,
	);
} finally {
	rmSync(root, { recursive: true, force: true });
}
