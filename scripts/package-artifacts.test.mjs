import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { appendFileSync, existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, parse } from "node:path";
import test from "node:test";
import { produceArtifactSet, readArtifactSet, verifyArtifactSet } from "./package-artifacts.mjs";
import { installConsumer, smokeTestNpmConsumer } from "./local-package-install.mjs";

function writePackage(directory, manifest, files) {
	mkdirSync(directory, { recursive: true });
	writeFileSync(join(directory, "package.json"), `${JSON.stringify(manifest, null, "\t")}\n`);
	for (const [path, contents] of Object.entries(files)) {
		mkdirSync(dirname(join(directory, path)), { recursive: true });
		writeFileSync(join(directory, path), contents);
	}
}

test("produces a verified, content-addressed artifact set", (t) => {
	const temporaryRoot = mkdtempSync(join(tmpdir(), "pi-package-artifacts-test-"));
	t.after(() => rmSync(temporaryRoot, { recursive: true, force: true }));
	const repoRoot = join(temporaryRoot, "repo with spaces");
	mkdirSync(repoRoot);
	writeFileSync(join(repoRoot, "package.json"), '{"name":"fixture","private":true}\n');
	writePackage(
		join(repoRoot, "packages", "shared"),
		{ name: "@pi-package-test/shared", version: "1.0.0", files: ["dist"] },
		{ "dist/index.js": 'export const marker = "artifact";\n' },
	);
	writePackage(
		join(repoRoot, "packages", "target"),
		{ name: "@pi-package-test/target", version: "1.0.0", files: ["dist"] },
		{ "dist/index.js": "export const target = true;\n" },
	);
	execFileSync("git", ["init", "--quiet"], { cwd: repoRoot });
	execFileSync("git", ["config", "user.email", "test@example.com"], { cwd: repoRoot });
	execFileSync("git", ["config", "user.name", "Test"], { cwd: repoRoot });
	execFileSync("git", ["add", "."], { cwd: repoRoot });
	execFileSync("git", ["commit", "--quiet", "-m", "fixture"], { cwd: repoRoot });

	const artifactSet = produceArtifactSet({
		build: false,
		outDir: join(repoRoot, ".artifacts", "package set"),
		repoRoot,
	});
	assert.deepEqual(
		artifactSet.packages.map((pkg) => pkg.name),
		["@pi-package-test/shared", "@pi-package-test/target"],
	);
	for (const pkg of artifactSet.packages) {
		assert.match(pkg.tarball, /-[0-9a-f]{12}\.tgz$/);
		assert.match(pkg.integrity, /^sha512-/);
	}
	assert.equal(artifactSet.source.dirty, false);
	assert.equal(readArtifactSet(artifactSet.manifestPath).packages.length, 2);

	writeFileSync(join(repoRoot, "packages/shared/dist/index.js"), 'export const marker = "changed";\n');
	const changedArtifactSet = produceArtifactSet({
		build: false,
		outDir: join(repoRoot, ".artifacts", "changed package set"),
		repoRoot,
	});
	assert.equal(changedArtifactSet.source.commit, artifactSet.source.commit);
	assert.equal(changedArtifactSet.source.dirty, true);
	assert.notEqual(
		changedArtifactSet.getPackage("@pi-package-test/shared").tarball,
		artifactSet.getPackage("@pi-package-test/shared").tarball,
	);

	appendFileSync(artifactSet.packages[0].tarballPath, "corrupt");
	assert.throws(() => readArtifactSet(artifactSet.manifestPath), /integrity mismatch/);
	const packageJsonPath = join(repoRoot, "packages", "shared", "package.json");
	assert.throws(
		() => produceArtifactSet({ build: false, force: true, outDir: join(repoRoot, "packages", "shared"), repoRoot }),
		/Repository-local output directory must be inside.*\.artifacts/,
	);
	assert.equal(existsSync(packageJsonPath), true);
	assert.throws(
		() => produceArtifactSet({ build: false, force: true, outDir: repoRoot, repoRoot }),
		/repository, its ancestor, or a filesystem root/,
	);
	assert.throws(
		() => produceArtifactSet({ build: false, force: true, outDir: temporaryRoot, repoRoot }),
		/repository, its ancestor, or a filesystem root/,
	);
	assert.throws(
		() => produceArtifactSet({ build: false, force: true, outDir: parse(repoRoot).root, repoRoot }),
		/repository, its ancestor, or a filesystem root/,
	);
});

test("packs only explicitly selected staged identities and validates source provenance", (t) => {
	const root = mkdtempSync(join(tmpdir(), "pi-selected-artifacts-test-"));
	t.after(() => rmSync(root, { recursive: true, force: true }));
	const repoRoot = join(root, "repo");
	mkdirSync(repoRoot);
	writeFileSync(join(repoRoot, "package.json"), '{"private":true}\n');
	writePackage(join(repoRoot, "packages", "upstream"), { name: "@pi-package-test/upstream", version: "1.0.0" }, {});
	const directory = join(root, "staged");
	writePackage(directory, { name: "@pi-package-test/fork", version: "1.0.0" }, {});
	const source = { commit: "a".repeat(40), dirty: true };
	const artifactSet = produceArtifactSet({
		repoRoot,
		outDir: join(root, "artifacts"),
		build: false,
		source,
		packages: [{ name: "@pi-package-test/fork", directory }],
	});
	assert.deepEqual(
		artifactSet.packages.map((pkg) => pkg.name),
		["@pi-package-test/fork"],
	);
	assert.deepEqual(artifactSet.source, source);
	const manifest = JSON.parse(readFileSync(artifactSet.manifestPath, "utf8"));
	manifest.source = { ...source, dirty: false };
	writeFileSync(artifactSet.manifestPath, JSON.stringify(manifest));
	assert.throws(() => verifyArtifactSet(artifactSet), /manifest changed after verification/);
	manifest.source = { commit: source.commit };
	writeFileSync(artifactSet.manifestPath, JSON.stringify(manifest));
	assert.throws(() => readArtifactSet(artifactSet.manifestPath), /Invalid package artifact source/);
	manifest.source = source;
	manifest.packages.push(manifest.packages[0]);
	writeFileSync(artifactSet.manifestPath, JSON.stringify(manifest));
	assert.throws(() => readArtifactSet(artifactSet.manifestPath), /Duplicate package/);
});

test("materializes only selected GitHub package bundles for isolated artifact installs", (t) => {
	const root = mkdtempSync(join(tmpdir(), "pi-selected-bundles-test-"));
	t.after(() => rmSync(root, { recursive: true, force: true }));
	const repoRoot = join(root, "repo");
	mkdirSync(repoRoot);
	writeFileSync(join(repoRoot, "package.json"), '{"private":true}\n');
	const sharedDirectory = join(repoRoot, "packages/shared");
	const shared = {
		name: "@pi-package-test/shared",
		version: "1.0.0",
		type: "module",
		main: "./dist/index.js",
		types: "./dist/index.d.ts",
		files: ["dist"],
	};
	writePackage(sharedDirectory, shared, {
		"dist/index.js": 'export const marker = "selected bundle";\n',
		"dist/index.d.ts": 'export declare const marker: "selected bundle";\n',
	});
	const directory = join(repoRoot, "packages/selected");
	const selected = {
		name: "@pi-package-test/selected",
		version: "1.0.0",
		type: "module",
		main: "./dist/index.js",
		types: "./dist/index.d.ts",
		files: ["dist"],
		dependencies: { [shared.name]: "1.0.0" },
		bundleDependencies: [shared.name],
		publishConfig: { registry: "https://npm.pkg.github.com" },
	};
	writePackage(directory, selected, {
		"dist/index.js": 'export { marker } from "@pi-package-test/shared";\n',
		"dist/index.d.ts": 'export { marker } from "@pi-package-test/shared";\n',
	});
	const unselectedDirectory = join(repoRoot, "packages/unselected");
	writePackage(
		unselectedDirectory,
		{
			name: "@pi-package-test/unselected",
			version: "1.0.0",
			bundleDependencies: ["@pi-package-test/missing"],
			publishConfig: { registry: "https://npm.pkg.github.com" },
		},
		{},
	);
	const artifactSet = produceArtifactSet({
		repoRoot,
		build: false,
		outDir: join(root, "artifacts"),
		source: null,
		packages: [{ name: selected.name, directory }],
	});
	assert.deepEqual(
		artifactSet.packages.map((pkg) => pkg.name),
		[selected.name],
	);
	assert.equal(existsSync(join(unselectedDirectory, "node_modules")), false);
	const consumer = join(root, "consumer");
	installConsumer({ artifactSet, directory: consumer, packageNames: [selected.name] });
	smokeTestNpmConsumer({ artifactSet, directory: consumer, packageName: selected.name });
	const marker = execFileSync(
		process.execPath,
		[
			"--input-type=module",
			"--eval",
			`import(${JSON.stringify(selected.name)}).then(({ marker }) => console.log(marker))`,
		],
		{ cwd: consumer, encoding: "utf8" },
	);
	assert.equal(marker.trim(), "selected bundle");
	writeFileSync(
		join(sharedDirectory, "package.json"),
		JSON.stringify({ ...shared, dependencies: { external: "1.0.0" } }),
	);
	const failedOutput = join(root, "failed-artifacts");
	assert.throws(
		() =>
			produceArtifactSet({
				repoRoot,
				build: false,
				outDir: failedOutput,
				source: null,
				packages: [{ name: selected.name, directory }],
			}),
		/must declare external@1.0.0 for bundled/,
	);
	assert.equal(existsSync(failedOutput), false);
});
