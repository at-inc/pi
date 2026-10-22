import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { test } from "node:test";
import YAML from "yaml";

const workflow = YAML.parse(
	readFileSync(new URL("../.github/workflows/publish-model-catalog.yml", import.meta.url), "utf8"),
);

test("catalog generation from a selected ref cannot save a default-branch package cache", () => {
	const setup = workflow.jobs.generate.steps.find((step) => step.uses?.startsWith("actions/setup-node@"));
	assert.equal(setup.with["package-manager-cache"], false);
	assert.equal(setup.with.cache, undefined);
});

test("manual, scheduled, and PR runs check out only their workflow revision", () => {
	assert.equal(workflow.on.workflow_dispatch.inputs.source_ref, undefined);
	assert.equal(workflow.on.workflow_dispatch.inputs.publish.type, "boolean");
	assert.deepEqual(workflow.on.schedule, [{ cron: "17 * * * *" }]);
	assert.ok(workflow.on.pull_request.paths.includes("packages/ai/**"));
	for (const job of Object.values(workflow.jobs)) {
		assert.equal(job.env?.SOURCE_REF, undefined);
		const checkouts = job.steps.filter((step) => step.uses?.startsWith("actions/checkout@"));
		assert.equal(checkouts.length, 1);
		assert.equal(checkouts[0].if, undefined);
		assert.equal(checkouts[0].with.ref, "${{ github.sha }}");
		assert.equal(checkouts[0].with["persist-credentials"], false);
	}
});

test("CI-triggered publication requires a successful main push in this repository", () => {
	assert.equal(
		workflow.jobs.generate.if,
		"${{ github.event_name != 'workflow_run' || (github.event.workflow_run.conclusion == 'success' && github.event.workflow_run.event == 'push' && github.event.workflow_run.head_branch == 'main' && github.event.workflow_run.head_repository.full_name == github.repository && github.event.workflow_run.head_sha == github.sha) }}",
	);
	assert.equal(workflow.jobs.publish.needs, "generate");
	assert.equal(workflow.jobs.publish.environment, "pi-model-upload");
	assert.equal(
		workflow.jobs.publish.if,
		"${{ github.event_name == 'schedule' || github.event_name == 'workflow_run' || (github.event_name == 'workflow_dispatch' && inputs.publish) }}",
	);
});
