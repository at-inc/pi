import assert from "node:assert/strict";
import crypto from "node:crypto";
import { mkdtemp, rm } from "node:fs/promises";
import { syncBuiltinESMExports } from "node:module";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { test } from "node:test";
import { Forwards } from "../../plugins/ports/store.ts";

test("forward IDs use uniform alphabet indexes and retry collisions", async (t) => {
	const directory = await mkdtemp(join(tmpdir(), "p2-forward-ids-"));
	t.after(() => rm(directory, { recursive: true, force: true }));
	let draws = 0;
	const random = t.mock.method(crypto, "randomInt", (max: number) => {
		assert.equal(max, 36);
		return draws++ < 20 ? 0 : 35;
	});
	syncBuiltinESMExports();
	t.after(() => {
		random.mock.restore();
		syncBuiltinESMExports();
	});
	const forwards = await Forwards.open(directory);
	const first = await forwards.add(3000, undefined, []);
	const second = await forwards.add(3001, undefined, []);
	assert.equal(first.id, "aaaaaaaaaa");
	assert.equal(second.id, "9999999999");
	assert.equal(draws, 30);
	assert.deepEqual((await Forwards.open(directory)).list(), [first, second]);
});
