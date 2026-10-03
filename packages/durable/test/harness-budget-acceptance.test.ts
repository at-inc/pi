import {
	createAssistantMessageEventStream,
	createModels,
	fauxAssistantMessage,
	type Model,
	type SimpleStreamOptions,
	type TranscriptContext,
} from "@at-inc/pi-ai";
import { clampMaxTokensToContext } from "@at-inc/pi-ai/api/simple-options";
import { estimateContextTokens } from "@at-inc/pi-ai/utils/estimate";
import { normalizeContext } from "@at-inc/pi-ai/utils/transcript";
import {
	AssistantEntry,
	CompactionEntry,
	CompactionTask,
	type EntryId,
	GenerationTask,
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

async function fixture(contextWindow = 262_144, maxTokens = 16_384) {
	const setup = chatSetup({
		models: [
			{ id: "faux-1", contextWindow, maxTokens },
			{ id: "small", contextWindow: 4_096, maxTokens: 16_384 },
		],
	});
	setup.settings.compaction = { enabled: true, reserveTokens: 16_384, keepRecentTokens: 1, backgroundTokens: 0 };
	setup.settings.retry = { enabled: false };
	setup.settings.minimumAnswerTokens = 1_024;
	const opened = await openChat(new MemoryStorage(), setup);
	owners.push(opened.harness);
	const requests: { model: Model<string>; transcript: TranscriptContext; options?: SimpleStreamOptions }[] = [];
	setup.faux.setResponses(
		Array.from({ length: 10 }, () => (transcript, options, _state, model) => {
			requests.push({ model, transcript, options });
			return fauxAssistantMessage("Checkpoint.");
		}),
	);
	let firstKept: EntryId | undefined;
	addHooks(setup.registry, CompactionTask, {
		beforeCompact: (selected) => {
			firstKept = selected.firstKept;
		},
	});
	async function history(inputTokens = 0) {
		await opened.root.commit(async (tx) => {
			for (const [index, text] of ["finish the migration", "Preserve the work log. ".repeat(200)].entries()) {
				await tx.appendEntry(UserEntry, opened.root.id, {
					model: [{ role: "user", content: text, timestamp: 1 + index * 2 }],
				});
				const response = fauxAssistantMessage("Ready to continue.", { timestamp: 2 + index * 2 });
				await tx.appendEntry(AssistantEntry, opened.root.id, {
					model: [{ ...response, usage: { ...response.usage, input: inputTokens, totalTokens: inputTokens } }],
				});
			}
		}, context);
	}
	return { ...opened, setup, requests, history, firstKept: () => firstKept };
}

describe("native request and placement acceptance", () => {
	it.each(["expansion", "live model switch"])("rejects summary %s below the pinned output cap", async (change) => {
		const { harness, root, setup, requests, history } = await fixture();
		await history();
		if (change === "live model switch")
			await root.commit(async (tx) => {
				await tx.appendEntry(UserEntry, root.id, {
					model: [{ role: "user", content: "u".repeat(40_000), timestamp: 5 }],
				});
				await tx.appendEntry(AssistantEntry, root.id, { model: [fauxAssistantMessage("Done.", { timestamp: 6 })] });
			}, context);
		const original = await root.context(context);
		const before = await allEntries(root);
		const started = Promise.withResolvers<void>();
		const release = Promise.withResolvers<void>();
		setup.faux.setResponses([
			async (transcript, options, _state, model) => {
				requests.push({ model, transcript, options });
				started.resolve();
				await release.promise;
				return fauxAssistantMessage("x".repeat(10_000));
			},
		]);
		const taskId = await root.compact(undefined, context);
		await started.promise;
		try {
			if (change === "live model switch")
				await root.configure({ model: { provider: "faux", modelId: "small" } }, context);
		} finally {
			release.resolve();
		}
		const task = await harness.waitForTask(taskId, context);
		expect(requests[0]!.options!.maxTokens).toBe(13_107);
		expect((await harness.snapshot(UsageDoc, root.id, context))?.models["faux/faux-1"]?.output).toBe(2_500);
		expect(task.state.outcome.status).toBe("failed");
		expect(await root.context(context)).toEqual(original);
		expect(await allEntries(root)).toEqual(before);
	});

	it.each([190_000, 10])("rejects oversized user input with %i reported history tokens", async (usage) => {
		const { root, requests, history } = await fixture(200_000);
		await history(usage);
		const original = await allEntries(root);
		const incoming = "u".repeat(800_000);
		const submitted = await root.submit({ type: "input", requestId: "oversized", content: incoming }, context);
		const result = await submitted.wait(context);
		expect(requests).toEqual([]);
		expect(result).toMatchObject({ status: "unanswered", requestId: "oversized" });
		expect((await allEntries(root)).slice(0, original.length)).toEqual(original);
		expect((await root.context(context)).messages).toContainEqual({
			role: "user",
			content: incoming,
			timestamp: expect.any(Number),
		});
		expect((await root.context(context)).head).toBeUndefined();
	});

	it("rejects a no-cut request when the public output clamp is one", async () => {
		const { root, setup, requests } = await fixture(200_000);
		setup.settings.compaction = { ...setup.settings.compaction, keepRecentTokens: 500_000 };
		await root.commit(async (tx) => {
			const response = fauxAssistantMessage("Nothing else to compact", { timestamp: 2 });
			await tx.appendEntry(AssistantEntry, root.id, {
				model: [{ ...response, usage: { ...response.usage, input: 196_000, totalTokens: 196_000 } }],
			});
		}, context);
		const content = "Check the migration results";
		const preview = normalizeContext({
			messages: [...(await root.context(context)).messages, { role: "user", content, timestamp: 3 }],
		});
		expect(clampMaxTokensToContext(setup.faux.getModel(), preview, 16_384)).toBe(1);
		const input = await root.submit({ type: "input", content }, context);
		expect((await input.wait(context)).status).toBe("unanswered");
		expect(requests).toEqual([]);
		expect(
			(await root.context(context)).messages.some(
				(message) => message.role === "user" && message.content === content,
			),
		).toBe(true);
	});

	it("rejects irreducible instructions before calling the provider", async () => {
		const { root, requests } = await fixture(22_768);
		await root.configure({ instructions: "s".repeat(80_000) }, context);
		const input = await root.submit({ type: "input", content: "Start" }, context);
		expect((await input.wait(context)).status).toBe("unanswered");
		expect(requests).toEqual([]);
		expect((await root.agent(context)).instructions).toBe("s".repeat(80_000));
	});

	it.each(["model", "caller"])("honors a deliberately smaller %s output ceiling", async (ceiling) => {
		const { root, setup, requests } = await fixture(262_144, ceiling === "model" ? 32 : 16_384);
		if (ceiling === "caller") {
			const options = { maxTokens: 32, cacheRetention: "none" } satisfies SimpleStreamOptions;
			setup.settings.stream = options;
		}
		const input = await root.submit({ type: "input", content: "A short answer is intentional." }, context);
		expect((await input.wait(context)).status).toBe("done");
		expect(requests).toHaveLength(1);
		if (ceiling === "caller") expect(requests[0]!.options!.maxTokens).toBe(32);
		expect(clampMaxTokensToContext(requests[0]!.model, requests[0]!.transcript, 32)).toBe(32);
	});

	it("allows a roomful no-cut request with 190K reported usage", async () => {
		const { root, setup, requests } = await fixture(200_000);
		setup.settings.compaction = { ...setup.settings.compaction, keepRecentTokens: 500_000 };
		await root.commit(async (tx) => {
			const response = fauxAssistantMessage("Nothing else to compact", { timestamp: 2 });
			await tx.appendEntry(AssistantEntry, root.id, {
				model: [{ ...response, usage: { ...response.usage, input: 190_000, totalTokens: 190_000 } }],
			});
		}, context);
		const input = await root.submit({ type: "input", content: "Check the migration results" }, context);
		expect((await input.wait(context)).status).toBe("done");
		expect(requests).toHaveLength(1);
		expect(clampMaxTokensToContext(requests[0]!.model, requests[0]!.transcript, 16_384)).toBeGreaterThanOrEqual(
			1_024,
		);
	});

	it("rechecks the normalized request after a hook replaces its messages", async () => {
		const { root, setup, requests } = await fixture(22_768);
		addHooks(setup.registry, GenerationTask, {
			beforeRequest: () => ({ messages: [{ role: "user", content: "x".repeat(100_000), timestamp: 1 }] }),
		});
		const input = await root.submit(
			{ type: "input", content: "Keep this canonical prompt", requestId: "hook-budget" },
			context,
		);
		expect(await input.wait(context)).toMatchObject({ status: "unanswered", requestId: "hook-budget" });
		expect(requests).toEqual([]);
		expect(
			(await root.context(context)).messages.some(
				(message) => message.role === "user" && message.content === "Keep this canonical prompt",
			),
		).toBe(true);
	});

	it("reads the live AgentDoc when a hook summary completes in the cached select phase", async () => {
		const { harness, root, setup, history, requests } = await fixture();
		await history();
		const original = await root.context(context);
		const started = Promise.withResolvers<void>();
		const release = Promise.withResolvers<void>();
		addHooks(setup.registry, CompactionTask, {
			beforeCompact: async () => {
				started.resolve();
				await release.promise;
				return { summary: "Checkpoint." };
			},
		});
		const taskId = await root.compact(undefined, context);
		await started.promise;
		await root.configure({ model: { provider: "faux", modelId: "small" } }, context);
		release.resolve();
		expect((await harness.waitForTask(taskId, context)).state.outcome.status).toBe("failed");
		expect(await root.context(context)).toEqual(original);
		expect(requests).toEqual([]);
	});

	it.each(["model", "instructions", "queued suffix", "later queued suffix", "queued input"])(
		"rechecks a queued summary after a concurrent %s change at the boundary",
		async (change) => {
			const { harness, root, setup, history } = await fixture();
			await history();
			const started = Promise.withResolvers<void>();
			const release = Promise.withResolvers<void>();
			setup.faux.setResponses([
				async () => {
					started.resolve();
					await release.promise;
					return fauxAssistantMessage("Ordinary answer.");
				},
				fauxAssistantMessage("Checkpoint."),
			]);
			const input = await root.submit({ type: "input", content: "Continue" }, context);
			await started.promise;
			let suffix: Awaited<ReturnType<typeof root.submit>> | undefined;
			if (change === "queued suffix")
				suffix = await root.submit(
					{
						type: "write",
						entry: {
							kind: "test.suffix",
							model: [{ role: "user", content: "x".repeat(1_100_000), timestamp: 10 }],
						},
					},
					context,
				);
			const task = await harness.waitForTask(await root.compact(undefined, context), context);
			expect(task.state.outcome.status).toBe("completed");
			if (task.state.outcome.status !== "completed") throw new Error("Expected summary admission");
			const submission = await harness.submission(task.state.outcome.result.submissionId!, context);
			expect((await submission!.status(context)).status).toBe("queued");
			if (change === "later queued suffix")
				suffix = await root.submit(
					{
						type: "write",
						entry: {
							kind: "test.later-suffix",
							model: [{ role: "user", content: "x".repeat(1_100_000), timestamp: 10 }],
						},
					},
					context,
				);
			if (change === "queued input")
				suffix = await root.submit({ type: "input", content: "x".repeat(1_100_000) }, context);
			if (change === "model") await root.configure({ model: { provider: "faux", modelId: "small" } }, context);
			if (change === "instructions") await root.configure({ instructions: "s".repeat(1_100_000) }, context);
			release.resolve();
			expect((await input.wait(context)).status).toBe("done");
			if (suffix !== undefined)
				expect((await suffix.wait(context)).status).toBe(change === "queued input" ? "unanswered" : "done");
			expect(await submission!.wait(context)).toMatchObject({ status: "unanswered", reason: "context_budget" });
			expect((await root.context(context)).head).toBeUndefined();
		},
	);

	it("places a reducing summary queued during a valid concurrent generation with the selected head", async () => {
		const { harness, root, setup, history, firstKept } = await fixture();
		await history();
		const started = Promise.withResolvers<void>();
		const release = Promise.withResolvers<void>();
		setup.faux.setResponses([
			async () => {
				started.resolve();
				await release.promise;
				return fauxAssistantMessage("Ordinary answer.");
			},
			fauxAssistantMessage("Checkpoint."),
		]);
		const input = await root.submit({ type: "input", content: "Continue" }, context);
		await started.promise;
		const task = await harness.waitForTask(await root.compact(undefined, context), context);
		expect(task.state.outcome.status).toBe("completed");
		if (task.state.outcome.status !== "completed") throw new Error("Expected summary admission");
		const submission = await harness.submission(task.state.outcome.result.submissionId!, context);
		expect((await submission!.status(context)).status).toBe("queued");
		release.resolve();
		expect((await input.wait(context)).status).toBe("done");
		expect((await submission!.wait(context)).status).toBe("done");
		expect((await root.context(context)).head).toMatchObject({ kind: CompactionEntry.kind, head: firstKept() });
		expect(estimateContextTokens((await root.context(context)).messages).tokens).toBeLessThan(2_000);
	});

	it("does not apply an uncompacted concurrent answer's usage to the newly placed prefix", async () => {
		const setup = chatSetup({
			models: [
				{ id: "faux-1", contextWindow: 262_144, maxTokens: 16_384 },
				{ id: "roomful", contextWindow: 22_768, maxTokens: 16_384 },
			],
		});
		setup.settings.minimumAnswerTokens = 1_024;
		setup.settings.compaction = { enabled: false, keepRecentTokens: 1 };
		const models = createModels();
		models.setProvider({
			...setup.faux.provider,
			streamSimple: (model, transcript, options) => {
				const stream = createAssistantMessageEventStream();
				const upstream = setup.faux.provider.streamSimple!(model, transcript, options);
				void upstream.result().then((message) => {
					const answer =
						transcript.messages[0]?.role !== "system"
							? { ...message, usage: { ...message.usage, input: 190_000, totalTokens: 190_000 } }
							: message;
					if (answer.stopReason !== "stop") throw new Error("Expected clean faux response");
					stream.push({ type: "done", reason: "stop", message: answer });
					stream.end();
				});
				return stream;
			},
		});
		const { harness, root } = await openChat(new MemoryStorage(), { ...setup, models });
		owners.push(harness);
		await root.commit(async (tx) => {
			await tx.appendEntry(UserEntry, root.id, {
				model: [{ role: "user", content: "u".repeat(20_000), timestamp: 1 }],
			});
			await tx.appendEntry(AssistantEntry, root.id, { model: [fauxAssistantMessage("Ready.", { timestamp: 2 })] });
		}, context);
		const started = Promise.withResolvers<void>();
		const release = Promise.withResolvers<void>();
		setup.faux.setResponses([
			async () => {
				started.resolve();
				await release.promise;
				return fauxAssistantMessage("Ordinary answer.");
			},
			fauxAssistantMessage("Checkpoint."),
			fauxAssistantMessage("Follow-up answer."),
		]);
		const input = await root.submit({ type: "input", content: "Continue" }, context);
		await started.promise;
		const task = await harness.waitForTask(await root.compact(undefined, context), context);
		if (task.state.outcome.status !== "completed") throw new Error("Expected summary admission");
		const submission = await harness.submission(task.state.outcome.result.submissionId!, context);
		expect((await submission!.status(context)).status).toBe("queued");
		await root.configure({ model: { provider: "faux", modelId: "roomful" } }, context);
		release.resolve();
		expect((await input.wait(context)).status).toBe("done");
		expect((await submission!.wait(context)).status).toBe("done");
		const projected = normalizeContext({ messages: [...(await root.context(context)).messages] });
		expect(clampMaxTokensToContext(setup.faux.getModel("roomful")!, projected, 16_384)).toBeGreaterThanOrEqual(1_024);
		const followUp = await root.submit({ type: "input", content: "Next" }, context);
		expect((await followUp.wait(context)).status).toBe("done");
	});
});
