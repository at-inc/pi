import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { test } from "node:test";
import YAML from "yaml";

test("catalog generation from a selected ref cannot save a default-branch package cache", () => {
	const workflow = YAML.parse(
		readFileSync(new URL("../.github/workflows/publish-model-catalog.yml", import.meta.url), "utf8"),
	);
	const setup = workflow.jobs.generate.steps.find((step) => step.uses?.startsWith("actions/setup-node@"));
	assert.equal(setup.with["package-manager-cache"], false);
	assert.equal(setup.with.cache, undefined);
});
