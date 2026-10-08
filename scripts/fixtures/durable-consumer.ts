import assert from "node:assert/strict";
import { mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { BACKGROUND_CONTEXT, withAbortSignal } from "@earendil-works/chord/context";
import { createModels, Type } from "@earendil-works/pi-ai";
import { fauxAssistantMessage, fauxProvider, fauxToolCall } from "@earendil-works/pi-ai/providers/faux";
import {
	AssistantEntry,
	CompactionEntry,
	CompactionTask,
	createRegistry,
	defineExtension,
	defineTool,
	Harness,
	type HarnessSettings,
	hook,
	section,
	ToolResultEntry,
} from "@earendil-works/pi-durable";
import { openNodeJsonlStorage } from "@earendil-works/pi-durable/storage/jsonl/node";

const context = withAbortSignal(AbortSignal.timeout(30_000), BACKGROUND_CONTEXT);
const summary = "## Goal\nVerify the beta SDK.\n## Progress\nEcho returned beta-smoke before the storage reopened.";
const faux = fauxProvider();
faux.setResponses([
	({ messages }) => {
		assert.equal(messages[0]?.role, "system");
		assert.equal(messages[1]?.role, "user");
		return fauxAssistantMessage([fauxToolCall("echo", { text: "beta-smoke" }, { id: "durable-beta-call" })], {
			stopReason: "toolUse",
		});
	},
	({ messages }) => {
		const result = messages.findLast((message) => message.role === "toolResult");
		assert.ok(result?.role === "toolResult");
		assert.equal(result.isError, false);
		assert.deepEqual(result.content, [{ type: "text", text: "beta-smoke" }]);
		return fauxAssistantMessage("durable-beta-ok");
	},
	({ messages }) => {
		assert.ok(
			messages.some((message) => message.role === "toolResult" && message.toolCallId === "durable-beta-call"),
		);
		return fauxAssistantMessage("durable-reopened-ok");
	},
	({ messages }, options) => {
		const system = messages[0];
		assert.ok(system?.role === "system" && typeof system.content === "string");
		assert.match(system.content, /context summarization assistant/);
		assert.match(JSON.stringify(messages), /Keep the echo result/);
		assert.match(JSON.stringify(messages), /\[Tool result\]: beta-smoke/);
		assert.equal(options?.cacheRetention, "none");
		return fauxAssistantMessage(summary);
	},
	({ messages }) => {
		assert.ok(
			messages.some(
				(message) =>
					message.role === "user" &&
					Array.isArray(message.content) &&
					message.content.some((block) => block.type === "text" && block.text.includes(summary)),
			),
		);
		assert.ok(messages.every((message) => message.role !== "toolResult"));
		return fauxAssistantMessage("durable-compacted-ok");
	},
]);
const models = createModels();
models.setProvider(faux.provider);
let executions = 0;
let compactions = 0;
const registry = createRegistry();
registry.install(
	defineExtension({
		name: "durable-beta-smoke",
		sections: [section("preamble", () => "Echo the requested text.")],
		tools: [
			defineTool({
				name: "echo",
				description: "Return the supplied text",
				parameters: Type.Object({ text: Type.String() }),
				replay: "safe",
				async execute(args, api) {
					assert.equal(api.models, models);
					assert.equal(api.models.getModel("faux", "faux-1")?.id, "faux-1");
					executions++;
					return { content: [{ type: "text", text: args.text }] };
				},
			}),
		],
		hooks: [
			hook(CompactionTask, {
				beforeCompact(compaction) {
					compactions++;
					assert.equal(compaction.reason, "manual");
					assert.equal(compaction.instructions, "Keep the echo result");
					assert.ok(compaction.entries.some((entry) => ToolResultEntry.is(entry)));
					return undefined;
				},
			}),
		],
	}),
);
const settings: HarnessSettings = {
	stream: { maxRetries: 0 },
	retry: { enabled: false },
	compaction: { enabled: false, reserveTokens: 1024, keepRecentTokens: 1, backgroundTokens: 0 },
};
const directory = await mkdtemp(join(tmpdir(), "pi-durable-consumer-"));
let harness: Harness | undefined;
try {
	harness = await Harness.open(
		await openNodeJsonlStorage(directory, context),
		{ models, registry, settings },
		context,
	);
	const root = await harness.root(context, { agent: { model: { provider: "faux", modelId: "faux-1" } } });
	const input = { type: "input", content: "Echo beta-smoke, then answer.", requestId: "durable-beta-input" } as const;
	const submission = await root.submit(input, context);
	const settled = await submission.wait(context);
	assert.ok(settled.status === "done" && settled.type === "input");
	const answer = await root.commit((tx) => tx.entry(AssistantEntry, settled.answer), context);
	assert.deepEqual(answer?.model?.[0]?.content, [{ type: "text", text: "durable-beta-ok" }]);
	assert.equal(executions, 1);
	assert.equal(faux.state.callCount, 2);
	const original = (await root.entries({}, 100, undefined, context)).items;
	assert.equal(original.filter((entry) => ToolResultEntry.is(entry)).length, 1);
	const toolResult = original.find((entry) => ToolResultEntry.is(entry));
	const resultMessage = toolResult?.model?.[0];
	assert.ok(resultMessage?.role === "toolResult" && typeof resultMessage.durationMs === "number");
	assert.ok(Number.isInteger(resultMessage.durationMs) && resultMessage.durationMs >= 0);
	assert.ok(toolResult?.byTaskId !== undefined);
	const toolTask = await harness.getTask(toolResult.byTaskId, context);
	assert.ok(toolTask?.state.status === "terminal");
	assert.ok(typeof toolTask.startedAt === "number" && typeof toolTask.endedAt === "number");
	assert.ok(toolTask.endedAt >= toolTask.startedAt);
	const initialContext = await root.context(context, { at: settled.answer });
	const ascending = await root.entries({ order: "ascending" }, 2, undefined, context);
	assert.deepEqual(ascending.items, original.slice(-2).reverse());
	assert.ok(ascending.next !== undefined);
	const remaining = await root.entries({}, 100, ascending.next, context);
	assert.deepEqual([...ascending.items, ...remaining.items], [...original].reverse());
	await assert.rejects(root.entries({ order: "descending" }, 100, ascending.next, context), /cursor continues/);
	await harness.close(BACKGROUND_CONTEXT);
	harness = undefined;

	harness = await Harness.open(
		await openNodeJsonlStorage(directory, context),
		{ models, registry, settings },
		context,
	);
	const reopened = await harness.root(context);
	assert.equal(reopened.id, root.id);
	assert.deepEqual((await reopened.entries({}, 100, undefined, context)).items, original);
	assert.deepEqual(await reopened.context(context, { at: settled.answer }), initialContext);
	assert.deepEqual(await harness.getTask(toolResult.byTaskId, context), toolTask);
	assert.deepEqual((await reopened.agent(context)).model, { provider: "faux", modelId: "faux-1" });
	const repeated = await reopened.submit(input, context);
	assert.equal(repeated.id, submission.id);
	assert.deepEqual(await repeated.wait(context), settled);
	assert.equal(faux.state.callCount, 2);
	const next = await (await reopened.submit({ type: "input", content: "Continue after reopening." }, context)).wait(
		context,
	);
	assert.ok(next.status === "done" && next.type === "input");
	const nextAnswer = await reopened.commit((tx) => tx.entry(AssistantEntry, next.answer), context);
	assert.deepEqual(nextAnswer?.model?.[0]?.content, [{ type: "text", text: "durable-reopened-ok" }]);
	const before = (await reopened.entries({}, 100, undefined, context)).items;

	const taskId = await reopened.compact("Keep the echo result", context);
	const { outcome } = (await harness.waitForTask(taskId, context)).state;
	assert.equal(outcome.status, "completed");
	assert.ok(outcome.status === "completed" && outcome.result.submissionId !== undefined);
	const placement = await harness.submission(outcome.result.submissionId, context);
	assert.ok(placement);
	assert.equal((await placement.wait(context)).status, "done");
	const compacted = await reopened.context(context);
	assert.deepEqual(await reopened.context(context, { at: settled.answer }), initialContext);
	assert.ok(CompactionEntry.is(compacted.head));
	assert.equal(compacted.head.data.reason, "manual");
	assert.equal(compacted.head.head, next.answer);
	assert.ok(compacted.entries.length < before.length);
	const after = (await reopened.entries({}, 100, undefined, context)).items;
	assert.equal(after.length, before.length + 1);
	assert.deepEqual(after.slice(1), before);
	assert.equal(compactions, 1);

	const last = await (await reopened.submit({ type: "input", content: "Continue from the summary." }, context)).wait(
		context,
	);
	assert.ok(last.status === "done" && last.type === "input");
	const lastAnswer = await reopened.commit((tx) => tx.entry(AssistantEntry, last.answer), context);
	assert.deepEqual(lastAnswer?.model?.[0]?.content, [{ type: "text", text: "durable-compacted-ok" }]);
	assert.equal(executions, 1);
	assert.equal(faux.state.callCount, 5);
	assert.equal(faux.getPendingResponseCount(), 0);
} finally {
	try {
		await harness?.close(BACKGROUND_CONTEXT);
	} finally {
		await rm(directory, { recursive: true, force: true });
	}
}
console.log(
	"Aliased Durable SDK smoke passed: one native tool execution with public model access and timings, ordered scans, as-of context, JSONL close/reopen and deduplication, native compaction, and five offline model responses.",
);
