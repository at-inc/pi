import { spawnSync } from "node:child_process";
import { mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { expect, test } from "vitest";

test("publishes the private public socket before accepting control registrations", () => {
	const directory = mkdtempSync(join(tmpdir(), "pi-coordinator-start-"));
	try {
		const script = join(directory, "startup.mjs");
		const coordinator = new URL("../src/experimental/coordinator.ts", import.meta.url).href;
		const resolver = fileURLToPath(new URL("../src/experimental/source-resolver.ts", import.meta.url));
		const publicPath = join(directory, "public.sock");
		const controlPath = join(directory, "control.sock");
		writeFileSync(
			script,
			`import assert from "node:assert/strict";
import fs from "node:fs";
import { syncBuiltinESMExports } from "node:module";
import { runCoordinatorProcess } from ${JSON.stringify(coordinator)};
const publicPath = ${JSON.stringify(publicPath)};
const controlPath = ${JSON.stringify(controlPath)};
const chmod = fs.promises.chmod;
let checked = false;
fs.promises.chmod = async (path, mode) => {
	if (path === controlPath) {
		const stats = await fs.promises.lstat(publicPath);
		assert.equal(stats.isSocket(), true);
		assert.equal(stats.mode & 0o777, 0o600);
		checked = true;
	}
	return chmod(path, mode);
};
syncBuiltinESMExports();
await runCoordinatorProcess([publicPath, controlPath]);
assert.equal(checked, true);
process.kill(process.pid, "SIGTERM");
`,
		);
		const result = spawnSync(process.execPath, ["--import", resolver, script], { encoding: "utf8", timeout: 10_000 });
		expect(result.error).toBeUndefined();
		expect(result.status, result.stderr).toBe(0);
	} finally {
		rmSync(directory, { recursive: true, force: true });
	}
});
