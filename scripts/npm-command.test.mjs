import assert from "node:assert/strict";
import test from "node:test";
import { parseNpmPackResult } from "./npm-command.mjs";

const packed = { name: "@pi-package-test/target", version: "1.0.0", filename: "pi-package-test-target-1.0.0.tgz" };

test("accepts array and npm 11.6+ name-keyed pack output", () => {
	assert.deepEqual(parseNpmPackResult(JSON.stringify([packed]), packed.name), packed);
	assert.deepEqual(parseNpmPackResult(JSON.stringify({ [packed.name]: packed }), packed.name), packed);
});

test("rejects ambiguous, missing, and mismatched pack results", () => {
	for (const result of [
		null,
		[],
		{},
		[packed, packed],
		{ wrong: packed },
		{ first: packed, second: packed },
		[{}],
		[{ ...packed, filename: "" }],
		[{ ...packed, name: "wrong" }],
	]) {
		assert.throws(() => parseNpmPackResult(JSON.stringify(result), packed.name), /unexpected result/);
	}
});
