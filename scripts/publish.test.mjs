import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import { chmodSync, existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";

const script = fileURLToPath(new URL("./publish.mjs", import.meta.url));

function publish(t, options = {}) {
	const root = mkdtempSync(join(tmpdir(), "pi-npm-publisher-test-"));
	t.after(() => rmSync(root, { recursive: true, force: true }));
	const bin = join(root, "bin");
	mkdirSync(bin);
	writeFileSync(join(root, "package.json"), '{"private":true}\n');
	for (const [name, privatePackage, registry] of [
		["@pi-package-test/npm", false, "https://registry.npmjs.org"],
		["@at-inc/pi", false, "https://npm.pkg.github.com"],
		["@pi-package-test/private", true, "https://registry.npmjs.org"],
	]) {
		const directory = join(root, "packages", name.split("/")[1]);
		mkdirSync(join(directory, "dist"), { recursive: true });
		writeFileSync(
			join(directory, "package.json"),
			JSON.stringify({
				name,
				version: "1.1.0",
				private: privatePackage,
				main: "./dist/index.js",
				types: "./dist/index.d.ts",
				publishConfig: { registry },
			}),
		);
		writeFileSync(join(directory, "dist/index.js"), "export {};\n");
		writeFileSync(join(directory, "dist/index.d.ts"), "export {};\n");
	}
	const git = join(bin, "git");
	writeFileSync(
		git,
		`#!${process.execPath}\nif (process.argv[2] === "rev-parse") console.log("${"a".repeat(40)}");\n`,
	);
	chmodSync(git, 0o755);
	const npm = join(bin, "npm");
	writeFileSync(
		npm,
		`#!${process.execPath}
const { appendFileSync, existsSync, readFileSync, writeFileSync } = require("node:fs");
const { createHash } = require("node:crypto");
const { dirname, join } = require("node:path");
const root = process.env.FIXTURE_ROOT;
const options = JSON.parse(process.env.FIXTURE_OPTIONS);
const args = process.argv.slice(2);
const manifest = args[0] === "view" ? undefined : JSON.parse(readFileSync(args[0] === "publish" ? args[1] : "package.json", "utf8"));
const integrity = args[0] === "publish" ? "sha512-" + createHash("sha512").update(readFileSync(args[1])).digest("base64") : undefined;
appendFileSync(join(root, "calls.jsonl"), JSON.stringify({ args, manifest, integrity, cwd: process.cwd() }) + "\\n");
if (args[0] === "pack") {
	const filename = "pi-package-test-npm-1.1.0.tgz";
	const destination = args[args.indexOf("--pack-destination") + 1];
	writeFileSync(join(destination, filename), JSON.stringify(manifest));
	writeFileSync(join(root, "artifact-directory.txt"), dirname(destination));
	const packed = { name: manifest.name, version: manifest.version, filename, size: 100, unpackedSize: 200,
		files: ["package.json", "dist/index.js", "dist/index.d.ts"].map(path => ({ path })) };
	console.log(JSON.stringify({ [manifest.name]: packed }));
} else if (args[0] === "view") {
	const directory = readFileSync(join(root, "artifact-directory.txt"), "utf8");
	const artifacts = JSON.parse(readFileSync(join(directory, "manifest.json"), "utf8"));
	writeFileSync(join(root, "artifacts.json"), JSON.stringify(artifacts));
	if (options.corrupt) appendFileSync(join(directory, artifacts.packages[0].tarball), "corrupt");
	if (options.published) console.log('"1.1.0"');
	else { console.error("E404 Not Found"); process.exit(1); }
} else if (args[0] !== "publish") process.exit(1);
`,
	);
	chmodSync(npm, 0o755);
	if (process.platform === "win32") {
		writeFileSync(join(bin, "npm.cmd"), `@"${process.execPath}" "${npm}" %*\r\n`);
		writeFileSync(join(bin, "git.cmd"), `@"${process.execPath}" "${git}" %*\r\n`);
	}
	const result = spawnSync(process.execPath, [script, ...(options.dryRun ? ["--dry-run"] : [])], {
		cwd: root,
		encoding: "utf8",
		env: {
			PATH: bin,
			npm_execpath: npm,
			FIXTURE_ROOT: root,
			FIXTURE_OPTIONS: JSON.stringify(options),
			SystemRoot: process.env.SystemRoot,
		},
	});
	const calls = readFileSync(join(root, "calls.jsonl"), "utf8").trim().split("\n").filter(Boolean).map(JSON.parse);
	return { ...result, calls, artifacts: JSON.parse(readFileSync(join(root, "artifacts.json"), "utf8")) };
}

for (const dryRun of [false, true]) {
	test(`npm publisher uses only its selected preflighted tarball${dryRun ? " in dry run" : ""}`, (t) => {
		const result = publish(t, { dryRun });
		assert.equal(result.status, 0, result.stderr);
		assert.deepEqual(
			result.artifacts.packages.map((pkg) => pkg.name),
			["@pi-package-test/npm"],
		);
		assert.deepEqual(result.artifacts.source, { commit: "a".repeat(40), dirty: false });
		const packed = result.calls.filter(({ args }) => args[0] === "pack");
		assert.equal(packed.length, 1);
		assert.equal(packed[0].manifest.name, "@pi-package-test/npm");
		const published = result.calls.filter(({ args }) => args[0] === "publish");
		assert.equal(published.length, 1);
		const call = published[0];
		assert.deepEqual(call.args, [
			"publish",
			call.args[1],
			"--access",
			"public",
			"--provenance",
			"--ignore-scripts",
			...(dryRun ? ["--dry-run"] : []),
		]);
		assert.ok(call.args[1].replaceAll("\\", "/").endsWith(result.artifacts.packages[0].tarball));
		assert.match(call.args[1], /-[0-9a-f]{12}\.tgz$/);
		assert.equal(call.integrity, result.artifacts.packages[0].integrity);
		assert.equal(
			call.integrity,
			`sha512-${createHash("sha512").update(JSON.stringify(packed[0].manifest)).digest("base64")}`,
		);
		assert.equal(existsSync(dirname(dirname(call.args[1]))), false);
	});
}

test("npm publisher skips existing immutable versions without repacking", (t) => {
	const result = publish(t, { published: true });
	assert.equal(result.status, 0, result.stderr);
	assert.equal(result.calls.filter(({ args }) => args[0] === "pack").length, 1);
	assert.equal(result.calls.filter(({ args }) => args[0] === "publish").length, 0);
	assert.match(result.stdout, /already published/);
});

test("npm publisher refuses a tarball changed after preflight", (t) => {
	const result = publish(t, { corrupt: true });
	assert.notEqual(result.status, 0);
	assert.equal(result.calls.filter(({ args }) => args[0] === "publish").length, 0);
	assert.match(result.stderr, /integrity mismatch/);
});
