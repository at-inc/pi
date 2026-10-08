import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { copyFile, mkdir, mkdtemp, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import test from "node:test";

async function check(t, target, utilities) {
	const root = await mkdtemp(join(tmpdir(), "pi-entry-graphs-"));
	t.after(() => rm(root, { recursive: true, force: true }));
	const files = {
		"packages/ai/package.json": JSON.stringify({
			exports: { "./models": "./dist/models.js", "./utils/*": target },
		}),
		"packages/ai/src/models.ts": "export {};\n",
		"packages/durable/package.json": JSON.stringify({ exports: { ".": "./dist/index.js" } }),
		"packages/durable/src/index.ts": "export {};\n",
		...Object.fromEntries(utilities.map((name) => [`packages/ai/src/utils/${name}.ts`, "export {};\n"])),
	};
	for (const [path, content] of Object.entries(files)) {
		const file = join(root, path);
		await mkdir(dirname(file), { recursive: true });
		await writeFile(file, content);
	}
	const script = join(root, "scripts/check-entry-graphs.mjs");
	await mkdir(dirname(script), { recursive: true });
	await copyFile(new URL("./check-entry-graphs.mjs", import.meta.url), script);
	return spawnSync(process.execPath, [script], { encoding: "utf8", timeout: 5_000 });
}

test("expands ordinary wildcard entries", async (t) => {
	const result = await check(t, "./dist/utils/*.js", ["first", "second"]);
	assert.equal(result.status, 0, result.stderr);
	assert.match(result.stdout, /Entry point graphs are within budget/);
});

test("preserves literal replacement metacharacters in source filenames", async (t) => {
	const result = await check(t, "./dist/utils/*.js", ["$&", "$`", "$'", "$$"]);
	assert.equal(result.status, 0, result.stderr);
});

test("replaces every wildcard occurrence in the export target", async (t) => {
	const result = await check(t, "./dist/utils/*-*.js", ["sample"]);
	assert.equal(result.status, 1, result.stderr);
	assert.match(result.stderr, /points at \.\/dist\/utils\/sample-sample\.js, which has no source file/);
	assert.doesNotMatch(result.stderr, /sample-\*/);
});
