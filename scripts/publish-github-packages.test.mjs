import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import {
	chmodSync,
	existsSync,
	mkdirSync,
	mkdtempSync,
	readFileSync,
	rmSync,
	symlinkSync,
	writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";

const script = fileURLToPath(new URL("./publish-github-packages.mjs", import.meta.url));
const registry = "https://npm.pkg.github.com";
const libraries = ["@at-inc/pi-ai", "@at-inc/pi-agent-core"];
const durablePackages = ["@at-inc/chord", "@at-inc/pi-durable"];
const sourceRevision = "1234567890123456789012345678901234567890";

function publish(t, options = {}) {
	const root = mkdtempSync(join(tmpdir(), "pi-publish-github-"));
	t.after(() => rmSync(root, { recursive: true, force: true }));
	const bin = join(root, "bin");
	mkdirSync(bin);
	symlinkSync(process.execPath, join(bin, process.platform === "win32" ? "node.exe" : "node"));
	writeFileSync(
		join(bin, "git"),
		`#!${process.execPath}\nif (process.argv[2] === "rev-parse") console.log(${JSON.stringify(sourceRevision)});\nelse if (${Boolean(options.dirty)}) console.log(" M fixture");\n`,
	);
	chmodSync(join(bin, "git"), 0o755);
	mkdirSync(join(root, "scripts"));
	writeFileSync(join(root, "scripts/prepare-github-package-bundles.mjs"), "");
	writeFileSync(join(root, "calls.jsonl"), "");
	const tags = Object.fromEntries(
		[...libraries, "@at-inc/pi", ...durablePackages].map((name) => [
			name,
			options.noLatest ? {} : { latest: "0.99.2" },
		]),
	);
	const originals = {};
	for (const [index, directory] of ["ai", "agent", "coding-agent", "chord", "durable"].entries()) {
		const path = join(root, "packages", directory);
		mkdirSync(path, { recursive: true });
		if (!(index === 1 && options.fault === "build")) {
			mkdirSync(join(path, "dist"));
			writeFileSync(join(path, "dist/index.js"), "export {};\n");
			writeFileSync(join(path, "dist/index.d.ts"), "export {};\n");
		}
		const manifest = {
			name: [...libraries, "@at-inc/pi", "@earendil-works/chord", "@earendil-works/pi-durable"][index],
			version: options.version ?? "1.0.0-beta.1",
			main: "./dist/index.js",
			types: "./dist/index.d.ts",
			repository: { url: "git+https://github.com/earendil-works/pi.git", directory: `packages/${directory}` },
			publishConfig: { registry },
		};
		if (index === 1 && options.fault === "version") manifest.version = "0.99.2";
		if (index === 1 && options.fault === "registry") manifest.publishConfig.registry = "https://registry.npmjs.org";
		if (index === 2 && options.cliVersion) manifest.version = options.cliVersion;
		if (index === 3) manifest.dependencies = { esbuild: "0.28.2" };
		if (index === 4) {
			manifest.dependencies = {
				"@earendil-works/chord": `^${manifest.version}`,
				"@at-inc/pi-ai": `^${manifest.version}`,
			};
			if (options.durableVersion) manifest.version = options.durableVersion;
		}
		if (options.published) tags[[...libraries, "@at-inc/pi", ...durablePackages][index]].beta = manifest.version;
		writeFileSync(join(path, "package.json"), JSON.stringify(manifest));
		originals[directory] = JSON.stringify(manifest);
	}
	writeFileSync(join(root, "tags.json"), JSON.stringify(tags));
	const fakeNpm = join(bin, "npm");
	writeFileSync(
		fakeNpm,
		`#!${process.execPath}
const { appendFileSync, existsSync, readFileSync, writeFileSync } = require("node:fs");
const { createHash } = require("node:crypto");
const { join } = require("node:path");
const root = process.env.FIXTURE_ROOT;
const options = JSON.parse(process.env.FIXTURE_OPTIONS);
const args = process.argv.slice(2);
const manifest = ["view", "dist-tag"].includes(args[0]) ? null : JSON.parse(readFileSync(args[0] === "publish" ? args[1] : "package.json", "utf8"));
const integrity = args[0] === "publish" ? "sha512-" + createHash("sha512").update(readFileSync(args[1])).digest("base64") : undefined;
appendFileSync(join(root, "calls.jsonl"), JSON.stringify({ args, name: manifest?.name, manifest, integrity, cwd: process.cwd() }) + "\\n");
const tagsFile = join(root, "tags.json");
const tags = JSON.parse(readFileSync(tagsFile, "utf8"));
if (args[0] === "view") {
	const artifactDirectoryFile = join(root, "artifact-directory.txt");
	if (existsSync(artifactDirectoryFile)) {
		const artifactDirectory = readFileSync(artifactDirectoryFile, "utf8");
		const artifactManifestPath = join(artifactDirectory, "manifest.json");
		if (existsSync(artifactManifestPath)) {
			const artifacts = JSON.parse(readFileSync(artifactManifestPath, "utf8"));
			writeFileSync(join(root, "artifacts.json"), JSON.stringify(artifacts));
			if (options.fault === "integrity") appendFileSync(join(artifactDirectory, artifacts.packages[0].tarball), "corrupt");
			if (options.fault === "provenance") {
				artifacts.source.dirty = !artifacts.source.dirty;
				writeFileSync(artifactManifestPath, JSON.stringify(artifacts));
			}
		}
	}
	if (options.fault === "auth") { console.error("E401 Unauthorized"); process.exit(1); }
	if (args[2] === "version") {
		const existingAi = options.args?.includes("--durable-only") && args[1].startsWith("@at-inc/pi-ai@") && !options.missingAi;
		if (!options.published && !existingAi) { console.error("E404 Not Found"); process.exit(1); }
		console.log(JSON.stringify(args[1].split("@").at(-1)));
	} else if (tags[args[1]]?.latest) console.log(JSON.stringify(tags[args[1]]));
} else if (args[0] === "dist-tag") {
	if (options.fault === "auth") { console.error("E401 Unauthorized"); process.exit(1); }
	const entries = Object.entries(tags[args[2]] ?? {});
	if (!entries.length) { console.error("E404 Not Found"); process.exit(1); }
	console.log(entries.map(([tag, version]) => tag + ": " + version).join("\\n"));
} else if (args[0] === "pack") {
	if (options.fault === "pack" && manifest.name.endsWith("agent-core")) process.exit(1);
	const files = options.fault === "contents" && manifest.name.endsWith("agent-core") ? [] :
		["package.json", "dist/index.js", "dist/index.d.ts"];
	const filename = manifest.name.replace(/^@/, "").replaceAll("/", "-") + "-" + manifest.version + ".tgz";
	const packed = { ...manifest, filename, files: files.map(path => ({ path })), size: 100, unpackedSize: 200 };
	if (options.fault === "pack-version" && manifest.name.endsWith("agent-core")) packed.version = "9.0.0";
	if (options.fault === "pack-name" && manifest.name.endsWith("agent-core")) packed.name = "@pi-package-test/wrong";
	if (!args.includes("--dry-run")) {
		const tarballDirectory = args[args.indexOf("--pack-destination") + 1];
		writeFileSync(join(tarballDirectory, filename), JSON.stringify(manifest));
		writeFileSync(join(root, "artifact-directory.txt"), join(tarballDirectory, ".."));
	}
	console.log(JSON.stringify(options.packObject ? { [manifest.name]: packed } : [packed]));
} else if (args[0] === "publish") {
	if (options.fault === "publish" && manifest.name.endsWith("agent-core")) {
		console.error("Fixture publication failed"); process.exit(1);
	}
	tags[manifest.name][args[args.indexOf("--tag") + 1]] = manifest.version;
	if (options.changeLatest) tags[manifest.name].latest = manifest.version;
	writeFileSync(tagsFile, JSON.stringify(tags));
} else { console.error("Unexpected npm command"); process.exit(1); }
`,
	);
	chmodSync(fakeNpm, 0o755);
	if (process.platform === "win32") {
		writeFileSync(join(bin, "npm.cmd"), `@"${process.execPath}" "${fakeNpm}" %*\r\n`);
		writeFileSync(join(bin, "git.cmd"), `@"${process.execPath}" "${join(bin, "git")}" %*\r\n`);
	}
	const result = spawnSync(process.execPath, [script, ...(options.args ?? ["--libraries-only"])], {
		cwd: root,
		encoding: "utf8",
		env: {
			PATH: bin,
			npm_execpath: fakeNpm,
			FIXTURE_ROOT: root,
			FIXTURE_OPTIONS: JSON.stringify(options),
			SystemRoot: process.env.SystemRoot,
		},
	});
	const calls = readFileSync(join(root, "calls.jsonl"), "utf8").trim().split("\n").filter(Boolean).map(JSON.parse);
	for (const [directory, original] of Object.entries(originals)) {
		assert.equal(readFileSync(join(root, "packages", directory, "package.json"), "utf8"), original);
	}
	if (existsSync(join(root, "artifact-directory.txt"))) {
		assert.equal(existsSync(readFileSync(join(root, "artifact-directory.txt"), "utf8")), false);
	}
	return {
		...result,
		calls,
		tags: JSON.parse(readFileSync(join(root, "tags.json"), "utf8")),
		published: calls.filter(({ args }) => args[0] === "publish"),
		artifacts: existsSync(join(root, "artifacts.json"))
			? JSON.parse(readFileSync(join(root, "artifacts.json"), "utf8"))
			: undefined,
	};
}

for (const [version, tag, args, names] of [
	["1.0.0-beta.1", "beta", ["--libraries-only"], libraries],
	["0.99.2-rev.1", "rev", [], [...libraries, "@at-inc/pi"]],
	["1.0.0", "latest", [], [...libraries, "@at-inc/pi"]],
]) {
	test(`publishes ${version} to ${tag} after validating every selected package`, (t) => {
		const result = publish(t, { version, args, cliVersion: args.length ? "0.1.0" : undefined });
		assert.equal(result.status, 0, result.stderr);
		assert.deepEqual(
			result.published.map(({ name }) => name),
			names,
		);
		const preflight = result.calls.slice(0, result.calls.indexOf(result.published[0]));
		assert.deepEqual(
			preflight.filter(({ args }) => args[0] === "pack").map(({ name }) => name),
			names,
		);
		assert.equal(preflight.filter(({ args }) => args[0] === "dist-tag").length, names.length);
		for (const call of result.published) {
			assert.deepEqual(call.args, [
				"publish",
				call.args[1],
				"--ignore-scripts",
				"--registry",
				registry,
				"--tag",
				tag,
			]);
			const artifact = result.artifacts.packages.find((pkg) => pkg.name === call.name);
			assert.ok(artifact);
			assert.ok(call.args[1].replaceAll("\\", "/").endsWith(artifact.tarball));
			assert.match(call.args[1], /-[0-9a-f]{12}\.tgz$/);
			assert.equal(call.integrity, artifact.integrity);
			assert.equal(
				call.integrity,
				`sha512-${createHash("sha512").update(JSON.stringify(call.manifest)).digest("base64")}`,
			);
			assert.equal(existsSync(call.args[1]), false);
			assert.equal(result.tags[call.name][tag], version);
			assert.equal(result.tags[call.name].latest, tag === "latest" ? version : "0.99.2");
		}
		assert.deepEqual(result.artifacts.source, { commit: sourceRevision, dirty: false });
		assert.deepEqual(result.artifacts.packages.map((pkg) => pkg.name).sort(), [...names].sort());
		assert.equal(result.calls.filter(({ args }) => args[0] === "pack").length, names.length);
		if (tag !== "latest") assert.match(result.stdout, /latest unchanged/);
	});
}

test("dry run validates both libraries without publishing or changing tags", (t) => {
	const result = publish(t, { args: ["--libraries-only", "--dry-run"] });
	assert.equal(result.status, 0, result.stderr);
	assert.equal(result.calls.filter(({ args }) => args[0] === "pack").length, 2);
	assert.equal(result.published.length, 0);
	assert.ok(result.calls.filter(({ args }) => args[0] === "pack").every(({ args }) => !args.includes("--dry-run")));
	assert.deepEqual(result.tags[libraries[0]], { latest: "0.99.2" });
});

test("accepts npm pack output keyed by package name", (t) => {
	const result = publish(t, { packObject: true });
	assert.equal(result.status, 0, result.stderr);
	assert.deepEqual(
		result.published.map(({ name }) => name),
		libraries,
	);
});

for (const version of ["1.0.0-rc.1", "1.0.0-latest.1", "1.0.0-alpha.1"]) {
	test(`refuses unsupported prerelease ${version}`, (t) => {
		const result = publish(t, { version });
		assert.notEqual(result.status, 0);
		assert.match(result.stderr, /Unsupported version or prerelease channel/);
		assert.equal(result.calls.length, 0);
	});
}

for (const fault of [
	"version",
	"registry",
	"build",
	"pack",
	"pack-version",
	"pack-name",
	"contents",
	"auth",
	"integrity",
	"provenance",
]) {
	test(`preflight ${fault} failure prevents every publish`, (t) => {
		const result = publish(t, { fault });
		assert.notEqual(result.status, 0);
		assert.equal(result.published.length, 0);
		if (fault === "integrity") assert.match(result.stderr, /integrity mismatch/);
		if (fault === "provenance") assert.match(result.stderr, /manifest changed after verification/);
	});
}

test("skips existing immutable versions explicitly", (t) => {
	const result = publish(t, { published: true });
	assert.equal(result.status, 0, result.stderr);
	assert.equal(result.published.length, 0);
	assert.match(result.stdout, /already published; skipping/);
});

test("records dirty source state without substituting another package selection", (t) => {
	const result = publish(t, { args: ["--libraries-only", "--dry-run"], dirty: true });
	assert.equal(result.status, 0, result.stderr);
	assert.deepEqual(result.artifacts.source, { commit: sourceRevision, dirty: true });
	assert.deepEqual(result.artifacts.packages.map((pkg) => pkg.name).sort(), [...libraries].sort());
});

test("fails verification if latest changes during a prerelease", (t) => {
	const result = publish(t, { changeLatest: true });
	assert.notEqual(result.status, 0);
	assert.match(result.stderr, /latest changed during prerelease publishing/);
});

test("retains the publication failure and verifies tags after a partial prerelease", (t) => {
	const result = publish(t, { fault: "publish" });
	assert.notEqual(result.status, 0);
	assert.match(result.stderr, /Fixture publication failed/);
	assert.match(result.stderr, /beta does not point to/);
	assert.equal(result.tags[libraries[0]].beta, "1.0.0-beta.1");
	assert.equal(result.tags[libraries[1]].beta, undefined);
	assert.equal(result.calls.filter(({ args }) => args[0] === "dist-tag").length, libraries.length * 2);
});

test("does not create latest when it was absent", (t) => {
	const result = publish(t, { noLatest: true });
	assert.equal(result.status, 0, result.stderr);
	for (const name of libraries) assert.equal(result.tags[name].latest, undefined);
});

test("publishes only staged fork Durable and Chord with exact dependencies and provenance", (t) => {
	const result = publish(t, { args: ["--durable-only"], noLatest: true });
	assert.equal(result.status, 0, result.stderr);
	assert.deepEqual(
		result.published.map(({ name }) => name),
		durablePackages,
	);
	for (const { name, manifest, cwd } of result.published) {
		assert.equal(manifest.gitHead, sourceRevision);
		assert.equal(manifest.repository.url, "git+https://github.com/at-inc/pi.git");
		assert.equal(manifest.publishConfig.registry, registry);
		assert.equal(result.tags[name].latest, undefined);
		assert.equal(result.tags[name].beta, "1.0.0-beta.1");
		assert.equal(existsSync(cwd), false);
	}
	assert.deepEqual(result.published[1].manifest.dependencies, {
		"@earendil-works/chord": "npm:@at-inc/chord@1.0.0-beta.1",
		"@at-inc/pi-ai": "1.0.0-beta.1",
	});
});

test("Durable dry run validates fork artifacts without publishing", (t) => {
	const result = publish(t, { args: ["--durable-only", "--dry-run"] });
	assert.equal(result.status, 0, result.stderr);
	assert.equal(result.published.length, 0);
	assert.ok(result.calls.some(({ args, name }) => args[0] === "pack" && name === "@at-inc/pi-durable"));
});

test("refuses Durable publication before its exact AI version exists", (t) => {
	const result = publish(t, { args: ["--durable-only"], missingAi: true });
	assert.notEqual(result.status, 0);
	assert.match(result.stderr, /Publish @at-inc\/pi-ai@1.0.0-beta.1 before/);
	assert.equal(result.published.length, 0);
});

test("refuses mismatched Durable source versions", (t) => {
	const result = publish(t, { args: ["--durable-only"], durableVersion: "1.0.0-beta.2" });
	assert.notEqual(result.status, 0);
	assert.match(result.stderr, /must match the fork beta/);
	assert.equal(result.published.length, 0);
});

test("refuses conflicting release selections", (t) => {
	const result = publish(t, { args: ["--durable-only", "--libraries-only"] });
	assert.notEqual(result.status, 0);
	assert.equal(result.calls.length, 0);
});

test("verifies existing prereleases without a latest tag or a second publish", (t) => {
	const result = publish(t, { args: ["--durable-only"], published: true, noLatest: true });
	assert.equal(result.status, 0, result.stderr);
	assert.equal(result.published.length, 0);
	assert.match(result.stdout, /beta verified; latest unchanged \(absent\)/);
	assert.ok(result.calls.some(({ args }) => args[0] === "dist-tag" && args[1] === "ls"));
});
