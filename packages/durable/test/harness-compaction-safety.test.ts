import { fauxAssistantMessage, type Message, type SimpleStreamOptions } from "@at-inc/pi-ai";
import { estimateContextTokens } from "@at-inc/pi-ai/utils/estimate";
import {
	AssistantEntry,
	CompactionTask,
	type EntryId,
	type Harness,
	MemoryStorage,
	UsageDoc,
	UserEntry,
} from "@earendil-works/pi-durable";
import { afterEach, describe, expect, it } from "vitest";
import { allEntries, chatSetup, openChat } from "./chat-support.ts";
import { addHooks } from "./harness-support.ts";
import { context } from "./session-support.ts";

const owners: Harness[] = [];

afterEach(async () => {
	for (const owner of owners.splice(0)) await owner.close(context);
});

async function fixture(contextWindow = 100_000) {
	const setup = chatSetup({ models: [{ id: "faux-1", contextWindow, maxTokens: 16_384 }] });
	setup.settings.compaction = { enabled: false, reserveTokens: 16_384, keepRecentTokens: 1, backgroundTokens: 0 };
	setup.settings.retry = { enabled: false };
	const opened = await openChat(new MemoryStorage(), setup);
	owners.push(opened.harness);
	await opened.root.commit(async (tx) => {
		for (let index = 0; index < 3; index++) {
			await tx.appendEntry(UserEntry, opened.root.id, {
				model: [{ role: "user", content: "u".repeat(4_000), timestamp: index * 2 }],
			});
			await tx.appendEntry(AssistantEntry, opened.root.id, {
				model: [fauxAssistantMessage("a".repeat(4_000))],
			});
		}
	}, context);
	return { ...opened, setup };
}

describe("compaction safety", () => {
	it("keeps tokenizer margin in the serialized request and pins the selected cut through placement", async () => {
		const { harness, root, setup } = await fixture(500_000);
		setup.settings.compaction = { ...setup.settings.compaction, keepRecentTokens: 20_000 };
		await root.commit(async (tx) => {
			for (let index = 0; index < 247; index++) {
				await tx.appendEntry(UserEntry, root.id, {
					model: [{ role: "user", content: "u".repeat(4_000), timestamp: index * 2 }],
				});
				await tx.appendEntry(AssistantEntry, root.id, { model: [fauxAssistantMessage("a".repeat(4_000))] });
			}
		}, context);
		const before = await allEntries(root);
		let firstKept: EntryId | undefined;
		addHooks(setup.registry, CompactionTask, {
			beforeCompact: (selected) => {
				firstKept = selected.firstKept;
			},
		});
		const requests: { messages: readonly Message[]; options: SimpleStreamOptions | undefined }[] = [];
		setup.faux.setResponses([
			(transcript, options) => {
				requests.push({ messages: transcript.messages, options });
				return fauxAssistantMessage("A safe checkpoint.");
			},
		]);
		const task = await harness.waitForTask(await root.compact("s".repeat(68_000), context), context);
		expect(task.state.outcome.status).toBe("completed");
		expect(requests).toHaveLength(1);
		const input = estimateContextTokens(requests[0]!.messages).tokens;
		const output = requests[0]!.options!.maxTokens!;
		expect(input * 1.05 + output).toBeLessThan(500_000);
		expect(500_000 - input - output).toBeGreaterThan(20_000);
		if (task.state.outcome.status !== "completed") throw new Error("Expected a completed summary");
		const submission = await harness.submission(task.state.outcome.result.submissionId!, context);
		expect((await submission!.wait(context)).status).toBe("done");
		expect((await root.context(context)).head?.head).toBe(firstKept);
		expect((await allEntries(root)).slice(0, before.length)).toEqual(before);
	});

	it("rejects a provider that ignores maxTokens without placing a summary and still records its spend", async () => {
		const { harness, root, setup } = await fixture();
		const before = await allEntries(root);
		const original = await root.context(context);
		const response = fauxAssistantMessage("s".repeat(60_000));
		response.usage.output = 15_000;
		setup.faux.setResponses([response]);
		const task = await harness.waitForTask(await root.compact(undefined, context), context);
		expect(task.state.outcome).toMatchObject({ status: "failed", error: { detail: { reason: "summary_budget" } } });
		expect(await allEntries(root)).toEqual(before);
		expect(await root.context(context)).toEqual(original);
		expect((await harness.snapshot(UsageDoc, root.id, context))?.models["faux/faux-1"]?.output).toBe(15_000);
	});

	it("applies the output budget to summaries supplied by beforeCompact too", async () => {
		const { harness, root, setup } = await fixture();
		const before = await allEntries(root);
		addHooks(setup.registry, CompactionTask, { beforeCompact: () => ({ summary: "s".repeat(60_000) }) });
		const task = await harness.waitForTask(await root.compact(undefined, context), context);
		expect(task.state.outcome).toMatchObject({ status: "failed", error: { detail: { reason: "summary_budget" } } });
		expect(await allEntries(root)).toEqual(before);
		expect((await root.context(context)).head).toBeUndefined();
	});

	it("fails before calling the model when no selected prefix can fit the summary request", async () => {
		const { harness, root, setup } = await fixture(1_024);
		const before = await allEntries(root);
		let calls = 0;
		setup.faux.setResponses([
			() => {
				calls++;
				return fauxAssistantMessage("Should not be requested.");
			},
		]);
		const task = await harness.waitForTask(await root.compact(undefined, context), context);
		expect(task.state.outcome).toMatchObject({ status: "failed", error: { detail: { reason: "summary_budget" } } });
		expect(calls).toBe(0);
		expect(await allEntries(root)).toEqual(before);
		expect((await root.context(context)).head).toBeUndefined();
	});
});
