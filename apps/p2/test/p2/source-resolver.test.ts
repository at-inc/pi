import assert from "node:assert/strict";
import { execFile } from "node:child_process";
import { copyFile, mkdir, mkdtemp, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { test } from "node:test";
import { promisify } from "node:util";

test("source aliases resolve fork packages alongside upstream packages and the plugin API", async (t) => {
	const directory = await mkdtemp(join(tmpdir(), "p2-source-resolver-"));
	t.after(() => rm(directory, { recursive: true, force: true }));
	const project = join(directory, "apps/p2");
	await mkdir(join(project, "src"), { recursive: true });
	await mkdir(join(directory, "packages/probe/src"), { recursive: true });
	await copyFile(new URL("../../src/source-resolver.ts", import.meta.url), join(project, "src/source-resolver.ts"));
	await writeFile(join(directory, "package.json"), '{"type":"module"}');
	await writeFile(join(directory, "packages/probe/src/index.ts"), 'export const value = "source";');
	await writeFile(join(project, "src/api.ts"), 'export const value = "api";');
	await writeFile(
		join(directory, "tsconfig.json"),
		JSON.stringify({
			compilerOptions: {
				paths: {
					"@at-inc/pi-ai": ["packages/probe/src/index.ts"],
					"@at-inc/pi-ai/*": ["packages/probe/src/*.ts"],
					"@at-inc/pi-agent-core": ["packages/probe/src/index.ts"],
					"@at-inc/pi": ["packages/probe/src/index.ts"],
					"@earendil-works/chord": ["packages/probe/src/index.ts"],
				},
			},
		}),
	);
	await writeFile(
		join(project, "tsconfig.json"),
		JSON.stringify({ compilerOptions: { paths: { "@p2/app/v1": ["src/api.ts"] } } }),
	);
	await writeFile(
		join(project, "src/probe.ts"),
		[
			'import { value as ai } from "@at-inc/pi-ai";',
			'import { value as subpath } from "@at-inc/pi-ai/index";',
			'import { value as agent } from "@at-inc/pi-agent-core";',
			'import { value as coding } from "@at-inc/pi";',
			'import { value as chord } from "@earendil-works/chord";',
			'import { value as api } from "@p2/app/v1";',
			"console.log(JSON.stringify([ai, subpath, agent, coding, chord, api]));",
		].join("\n"),
	);
	const { stdout } = await promisify(execFile)(
		process.execPath,
		["--import", "./src/source-resolver.ts", "src/probe.ts"],
		{ cwd: project },
	);
	assert.deepEqual(JSON.parse(stdout), ["source", "source", "source", "source", "source", "api"]);
});
