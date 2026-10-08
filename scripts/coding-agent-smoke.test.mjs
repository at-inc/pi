import assert from "node:assert/strict";
import { existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import test from "node:test";
import { installConsumer, smokeTestNpmConsumer } from "./local-package-install.mjs";
import { produceArtifactSet, readArtifactSet } from "./package-artifacts.mjs";
import { codingAgentName, smokeTestCodingAgent } from "./coding-agent-smoke.mjs";

const devPackages = ["pi-client", "pi-protocol", "pi-server"].map((name) => `@earendil-works/${name}`);

function writePackage(directory, manifest, files) {
	mkdirSync(directory, { recursive: true });
	writeFileSync(join(directory, "package.json"), `${JSON.stringify(manifest, null, "\t")}\n`);
	for (const [path, content] of Object.entries(files)) {
		mkdirSync(dirname(join(directory, path)), { recursive: true });
		writeFileSync(join(directory, path), content);
	}
}

function createFixture(t, { importServer = false, declareServer = false, bundleChord = false } = {}) {
	const temporaryRoot = mkdtempSync(join(tmpdir(), "pi-coding-agent-smoke-test-"));
	t.after(() => rmSync(temporaryRoot, { recursive: true, force: true }));
	const root = join(temporaryRoot, "fixture with spaces");
	const repoRoot = join(root, "repo");
	mkdirSync(repoRoot, { recursive: true });
	writeFileSync(join(repoRoot, "package.json"), '{"name":"fixture","private":true}\n');
	const packageNames = [codingAgentName, "@earendil-works/chord", ...devPackages];
	for (const name of packageNames) {
		const isAgent = name === codingAgentName;
		writePackage(
			join(repoRoot, "packages", name.split("/")[1]),
			{
				name,
				version: "1.0.0",
				type: "module",
				main: "./dist/index.js",
				types: "./dist/index.d.ts",
				exports: isAgent
					? {
							".": { types: "./dist/index.d.ts", import: "./dist/index.js" },
							"./client": { source: "./src/client/index.ts" },
							"./experimental/plugin": { source: "./src/experimental/plugin.ts" },
						}
					: { ".": { types: "./dist/index.d.ts", import: "./dist/index.js" } },
				files: ["dist", "docs", "examples", "README.md", "CHANGELOG.md"],
				...(isAgent
					? {
							bin: { pi: "dist/bundle/cli.js" },
							dependencies: {
								"@earendil-works/chord": "1.0.0",
								...(declareServer ? { "@earendil-works/pi-server": "1.0.0" } : {}),
							},
							devDependencies: Object.fromEntries(devPackages.map((packageName) => [packageName, "1.0.0"])),
							...(bundleChord ? { bundleDependencies: ["@earendil-works/chord"] } : {}),
						}
					: {}),
			},
			{
				"dist/index.js": isAgent
					? `${importServer ? 'import "@earendil-works/pi-server";' : ""}
import { marker } from "@earendil-works/chord";
if (marker !== ${JSON.stringify(bundleChord ? "bundled artifact" : "local tarball")}) throw new Error("Wrong Chord artifact");
export function createAgentSession() {}
export class SessionManager { static inMemory() {} }
export class ModelRuntime { static create() {} }
`
					: 'export const marker = "local tarball";\n',
				"dist/index.d.ts": "export {};\n",
				...(isAgent
					? {
							"dist/cli.js": 'console.log("1.0.0");\n',
							"dist/bundle/cli.js": 'console.log("1.0.0");\n',
							"README.md": "Fixture\n",
							"CHANGELOG.md": "Fixture\n",
							"docs/fixture.md": "Fixture\n",
							"examples/fixture.ts": "export {};\n",
							"dist/modes/interactive/theme/dark.json": "{}\n",
							"dist/modes/interactive/theme/light.json": "{}\n",
							"dist/modes/interactive/theme/theme-schema.json": "{}\n",
							"dist/modes/interactive/assets/clankolas.png": "fixture",
							"dist/core/export-html/template.html": "fixture",
							"dist/core/export-html/template.css": "fixture",
							"dist/core/export-html/template.js": "export {};\n",
							"dist/core/export-html/vendor/highlight.min.js": "export {};\n",
							"dist/core/export-html/vendor/marked.min.js": "export {};\n",
							...(bundleChord
								? {
										"node_modules/@earendil-works/chord/package.json": JSON.stringify({
											name: "@earendil-works/chord",
											version: "1.0.0",
											type: "module",
											main: "./dist/index.js",
											types: "./dist/index.d.ts",
											exports: "./dist/index.js",
										}),
										"node_modules/@earendil-works/chord/dist/index.js":
											'export const marker = "bundled artifact";\n',
										"node_modules/@earendil-works/chord/dist/index.d.ts":
											'export declare const marker: "bundled artifact";\n',
									}
								: {}),
						}
					: {}),
			},
		);
	}
	const artifactSet = produceArtifactSet({ build: false, outDir: join(root, "artifacts"), repoRoot, source: null });
	const directory = join(root, "consumer");
	const installArtifacts = bundleChord
		? { ...artifactSet, packages: artifactSet.packages.filter((pkg) => pkg.name === codingAgentName) }
		: artifactSet;
	installConsumer({ artifactSet: installArtifacts, directory, packageNames: [codingAgentName] });
	return directory;
}

// #9132: installing every tarball directly hid undeclared runtime imports.
test("accepts a valid coding-agent package and rejects development-only packages and files", (t) => {
	const directory = createFixture(t);
	const manifest = JSON.parse(readFileSync(join(directory, "package.json"), "utf8"));
	assert.deepEqual(Object.keys(manifest.dependencies), [codingAgentName]);
	for (const name of devPackages) {
		assert.ok(manifest.overrides[name]);
		assert.equal(existsSync(join(directory, "node_modules", name)), false);
	}
	smokeTestCodingAgent(directory);
	smokeTestCodingAgent(directory, "bun");

	const nested = join(directory, "node_modules", codingAgentName, "node_modules/@earendil-works/pi-server");
	mkdirSync(nested, { recursive: true });
	writeFileSync(join(nested, "package.json"), JSON.stringify({ name: "@earendil-works/pi-server", version: "1.0.0" }));
	assert.throws(() => smokeTestCodingAgent(directory), /pi-server must not be installed/);
	rmSync(nested, { recursive: true });

	const experimental = join(directory, "node_modules", codingAgentName, "dist/experimental");
	mkdirSync(experimental);
	assert.throws(() => smokeTestCodingAgent(directory), /contains development-only code/);
});

// #9132: smoke-test the public SDK, not just a bundled CLI that hides missing imports.
test("fails when the SDK imports an undeclared server despite a working CLI", (t) => {
	const directory = createFixture(t, { importServer: true });
	assert.throws(() => smokeTestCodingAgent(directory), /Cannot find package '@earendil-works\/pi-server'/);
});

test("fails if a development-only dependency is added back to the published dependency tree", (t) => {
	const directory = createFixture(t, { declareServer: true });
	assert.throws(() => smokeTestCodingAgent(directory), /pi-server must not be installed/);
});

test("rejects missing stable SDK declarations and runtime assets", (t) => {
	const directory = createFixture(t);
	const packageDirectory = join(directory, "node_modules", codingAgentName);
	const declarations = join(packageDirectory, "dist/index.d.ts");
	const contents = readFileSync(declarations, "utf8");
	rmSync(declarations);
	assert.throws(() => smokeTestCodingAgent(directory), /types does not exist/);
	writeFileSync(declarations, contents);
	rmSync(join(packageDirectory, "dist/modes/interactive/theme/dark.json"));
	assert.throws(() => smokeTestCodingAgent(directory), /missing a runtime asset.*dark.json/);
});

test("validates bundled runtime files through their host artifact without substituting development peers", (t) => {
	const directory = createFixture(t, { bundleChord: true });
	const artifactSet = readArtifactSet(join(directory, "..", "artifacts", "manifest.json"));
	assert.equal(existsSync(join(directory, "node_modules/@earendil-works/chord")), false);
	smokeTestNpmConsumer({ artifactSet, directory, packageName: codingAgentName });
	smokeTestCodingAgent(directory);
	const hostManifestPath = join(directory, "node_modules", codingAgentName, "package.json");
	const hostManifest = JSON.parse(readFileSync(hostManifestPath, "utf8"));
	delete hostManifest.bundleDependencies;
	writeFileSync(hostManifestPath, JSON.stringify(hostManifest));
	assert.throws(
		() => smokeTestNpmConsumer({ artifactSet, directory, packageName: codingAgentName }),
		/no verified bundle host/,
	);
});
