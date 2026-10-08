#!/usr/bin/env node

import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { copyFileSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { parseArgs } from "node:util";
import {
	installConsumer,
	verifyPackageFiles,
	verifyPnpmConsumerArtifacts,
	verifySingleRuntime,
} from "./local-package-install.mjs";
import { produceArtifactSet, readArtifactSet } from "./package-artifacts.mjs";
import { prepareDurablePackages } from "./prepare-durable-packages.mjs";
import { execPnpmSync } from "./npm-command.mjs";

const { values } = parseArgs({
	options: { registry: { type: "boolean", default: false }, manifest: { type: "string" } },
});
if (values.registry && values.manifest) throw new Error("Use either --registry or --manifest, not both");
const repo = process.cwd();
const packages = [
	{ name: "@at-inc/pi-ai", alias: "@earendil-works/pi-ai", directory: "packages/ai" },
	{ name: "@at-inc/pi-agent-core", alias: "@earendil-works/pi-agent-core", directory: "packages/agent" },
	{ name: "@at-inc/chord", alias: "@earendil-works/chord" },
	{ name: "@at-inc/pi-durable", alias: "@earendil-works/pi-durable" },
];
const root = mkdtempSync(join(tmpdir(), "pi-durable-install-"));
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
			const staged = prepareDurablePackages(join(root, "staged"));
			artifactSet = produceArtifactSet({
				repoRoot: repo,
				build: false,
				outDir: join(root, "artifacts"),
				packages: [...packages.slice(0, 2), ...staged],
			});
		}
		if (artifactSet.source === null) throw new Error("Durable verification requires artifact source provenance");
	}
	const version = artifactSet
		? artifactSet.getPackage("@at-inc/pi-ai").version
		: JSON.parse(readFileSync(join(repo, "packages/ai/package.json"), "utf8")).version;
	if (artifactSet && packages.some((pkg) => artifactSet.getPackage(pkg.name).version !== version)) {
		throw new Error("Durable packages must have matching versions");
	}
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
	const imports = [];
	for (const pkg of packages) {
		const { manifest, runtimeSpecifiers } = verifyPackageFiles({
			directory,
			packageName: pkg.alias,
			expectedName: pkg.name,
			version,
		});
		imports.push(
			...runtimeSpecifiers.map(
				(specifier) =>
					`import ${JSON.stringify(specifier)}${specifier.endsWith(".json") ? ' with { type: "json" }' : ""};`,
			),
		);
		if (pkg.name === "@at-inc/pi-durable") {
			assert.equal(manifest.dependencies["@earendil-works/chord"], `npm:@at-inc/chord@${version}`);
			assert.equal(manifest.dependencies["@at-inc/pi-ai"], version);
		}
		if (pkg.name === "@at-inc/chord" || pkg.name === "@at-inc/pi-durable") {
			assert.match(manifest.gitHead, /^[0-9a-f]{40}$/);
			if (artifactSet)
				assert.equal(
					manifest.gitHead,
					artifactSet.source.commit,
					`${pkg.name} source must match the immutable artifact manifest`,
				);
			else {
				execFileSync(
					"git",
					[
						"diff",
						"--exit-code",
						manifest.gitHead,
						"HEAD",
						"--",
						"packages/chord",
						"packages/durable",
						"tsconfig.base.json",
						"package.json",
						"package-lock.json",
					],
					{ cwd: repo, stdio: "inherit" },
				);
				execFileSync(
					"git",
					[
						"diff",
						"--exit-code",
						"HEAD",
						"--",
						"packages/chord",
						"packages/durable",
						"tsconfig.base.json",
						"package.json",
						"package-lock.json",
					],
					{ cwd: repo, stdio: "inherit" },
				);
			}
			console.log(
				`Verified ${pkg.name}@${version}: gitHead ${manifest.gitHead}; exports ${Object.keys(manifest.exports).join(", ")}`,
			);
		}
	}
	if (artifactSet) verifyPnpmConsumerArtifacts({ directory, artifactSet, packageNames, packageAliases });
	verifySingleRuntime({ directory, packageNames, runtimeNames: ["@earendil-works/pi-ai", "@earendil-works/chord"] });
	writeFileSync(join(directory, "exports.mjs"), `${imports.join("\n")}\n`);
	copyFileSync(new URL("./fixtures/durable-consumer.ts", import.meta.url), join(directory, "smoke.ts"));
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
	execFileSync(process.execPath, ["exports.mjs"], { cwd: directory, env, stdio: "inherit", timeout: 45_000 });
	for (const runtime of [process.execPath, "bun"]) {
		execFileSync(runtime, ["smoke.ts"], { cwd: directory, env, stdio: "inherit", timeout: 45_000 });
	}
	console.log(
		`Verified fork Durable and Chord ${version} from ${values.registry ? "GitHub Packages" : artifactSet.manifestPath} with single AI and Chord runtimes.`,
	);
} finally {
	rmSync(root, { recursive: true, force: true });
}
