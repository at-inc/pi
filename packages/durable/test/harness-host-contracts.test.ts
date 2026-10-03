import { mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fauxAssistantMessage, type Message, type SimpleStreamOptions } from "@at-inc/pi-ai";
import {
	AgentDoc,
	GenerationTask,
	type Harness,
	InboxDoc,
	MemoryStorage,
	type PreparedInputEntry,
} from "@earendil-works/pi-durable";
import { afterEach, describe, expect, it } from "vitest";
import { openNodeSqliteStorage } from "../src/storage/sqlite/node.ts";
import { allEntries, chatSetup, openChat, unanswered } from "./chat-support.ts";
import { addHooks } from "./harness-support.ts";
import { ControlledStorage, context } from "./session-support.ts";

const owners: Harness[] = [];
const directories: string[] = [];

afterEach(async () => {
	for (const owner of owners.splice(0)) await owner.close(context);
	for (const directory of directories.splice(0)) await rm(directory, { recursive: true, force: true });
});

function prepared() {
	return {
		kind: "app.input",
		model: [
			{ role: "user", content: "Channel: engineering", timestamp: 1 },
			{ role: "user", content: [{ type: "text", text: "Inspect the attachment" }], timestamp: 2 },
		] satisfies Message[],
		data: { display: { text: "Inspect the attachment", attachments: [{ id: "attachment-1" }] } },
	};
}

describe("prepared input entries", () => {
	it("admits the full projection, display data, receipt, and run in one commit and reuses the request", async () => {
		const setup = chatSetup();
		const requests: Message[][] = [];
		setup.faux.setResponses([
			(transcript) => {
				requests.push(transcript.messages);
				return fauxAssistantMessage("Done.");
			},
		]);
		const storage = new ControlledStorage();
		const { harness, root } = await openChat(storage, setup);
		owners.push(harness);
		const entry = prepared();
		const original = structuredClone(entry);
		const input = await root.submit(
			{ type: "input", content: "Inspect the attachment", requestId: "prepared", entry },
			context,
		);
		entry.data.display.attachments[0]!.id = "mutated";
		entry.model[1]!.content = "mutated";
		const receipt = await input.wait(context);
		expect(receipt.status).toBe("done");
		const stored = (await allEntries(root)).find((record) => record.id === receipt.entry)!;
		expect(stored).toMatchObject(original);
		expect(requests[0]).toEqual(original.model);
		const admission = storage.commits.find((batch) =>
			batch.some((write) => write.type === "submission" && write.value.id === input.id),
		)!;
		expect(admission.filter((write) => write.type === "entry")).toHaveLength(1);
		expect(admission.some((write) => write.type === "task" && write.value.kind === "pi.generation")).toBe(true);
		const count = storage.commits.length;
		const reused = await root.submit(
			{ type: "input", content: "different", requestId: "prepared", entry: prepared() },
			context,
		);
		expect(reused.id).toBe(input.id);
		expect(await reused.wait(context)).toEqual(receipt);
		expect(storage.commits).toHaveLength(count);
		const view = await root.viewState(context);
		expect(view.value.entries.find((record) => record.id === receipt.entry)?.data).toEqual(original.data);
		view.dispose();
	});

	it("deep-copies a queued prepared entry and places only that entry at the final boundary", async () => {
		const setup = chatSetup();
		const started = Promise.withResolvers<void>();
		const release = Promise.withResolvers<void>();
		setup.faux.setResponses([
			async () => {
				started.resolve();
				await release.promise;
				return fauxAssistantMessage("First answered.");
			},
			fauxAssistantMessage("Prepared answered."),
		]);
		const { harness, root } = await openChat(new MemoryStorage(), setup);
		owners.push(harness);
		const first = await root.submit({ type: "input", content: "Busy" }, context);
		await started.promise;
		const entry = prepared();
		const original = structuredClone(entry);
		const queued = await root.submit({ type: "input", content: "Inspect the attachment", entry }, context);
		expect((await queued.status(context)).status).toBe("queued");
		entry.data.display.text = "changed";
		entry.model[0]!.content = "changed";
		expect((await harness.snapshot(InboxDoc, root.id, context))?.items).toContainEqual({
			id: queued.id,
			mode: "followUp",
			content: "Inspect the attachment",
			entry: original,
		});
		release.resolve();
		expect((await first.wait(context)).status).toBe("done");
		const receipt = await queued.wait(context);
		expect(receipt.status).toBe("done");
		expect((await allEntries(root)).find((record) => record.id === receipt.entry)).toMatchObject(original);
		expect((await allEntries(root)).filter((record) => record.kind === "app.input")).toHaveLength(1);
	});

	it.each(["head", "edits", "byTaskId", "id"])(
		"rejects a prepared entry's forbidden %s without any write",
		async (key) => {
			const storage = new ControlledStorage();
			const { harness, root } = await openChat(storage, chatSetup());
			owners.push(harness);
			const count = storage.commits.length;
			const entry = { ...prepared(), [key]: key === "edits" ? [] : "self" } as unknown as PreparedInputEntry;
			await expect(root.submit({ type: "input", content: "No mutation", entry }, context)).rejects.toThrow(
				`Prepared input entry cannot set ${key}`,
			);
			expect(storage.commits).toHaveLength(count);
			expect(await allEntries(root)).toEqual([]);
		},
	);

	it.each([undefined, []])("rejects a missing or empty prepared model projection: %j", async (model) => {
		const storage = new ControlledStorage();
		const { harness, root } = await openChat(storage, chatSetup());
		owners.push(harness);
		const count = storage.commits.length;
		const entry = { kind: "app.input", ...(model === undefined ? {} : { model }) } as unknown as PreparedInputEntry;
		await expect(root.submit({ type: "input", content: "Keep input", entry }, context)).rejects.toThrow(
			"requires a non-empty model projection",
		);
		expect(storage.commits).toHaveLength(count);
	});

	it("preserves the prepared canonical entry and receipt on a budget rejection", async () => {
		const setup = chatSetup({ models: [{ id: "faux-1", contextWindow: 22_768, maxTokens: 16_384 }] });
		setup.settings.minimumAnswerTokens = 1_024;
		const { harness, root } = await openChat(new MemoryStorage(), setup);
		owners.push(harness);
		const entry = prepared();
		entry.model[1]!.content = "u".repeat(100_000);
		const input = await root.submit(
			{ type: "input", content: "Inspect the attachment", requestId: "budget-input", entry },
			context,
		);
		const receipt = await input.wait(context);
		expect(receipt).toMatchObject({ status: "unanswered", requestId: "budget-input", entry: expect.any(Number) });
		expect((await allEntries(root))[0]).toMatchObject(entry);
		expect(setup.faux.state.callCount).toBe(0);
	});
});

describe("ephemeral request options", () => {
	it("isolates session routing, headers, and payload callbacks between conversations and a fork of one Harness", async () => {
		const setup = chatSetup();
		setup.settings.stream = { headers: { shared: "base" } };
		const payloads: string[] = [];
		const requests: SimpleStreamOptions[] = [];
		addHooks(setup.registry, GenerationTask, {
			beforeRequest: ({ model, options }, api) => {
				expect(model.id).toBe("faux-1");
				expect(options.headers).toEqual({ shared: "base" });
				const id = `conversation:${api.conversationId}`;
				return {
					options: {
						sessionId: id,
						headers: { conversation: id },
						cacheRetention: "none",
						onPayload: (payload) => {
							payloads.push(id);
							return { original: payload, id };
						},
					},
				};
			},
		});
		setup.faux.setResponses(
			Array.from({ length: 3 }, () => async (_transcript, options) => {
				requests.push(options!);
				expect(await options!.onPayload!({ prompt: "synthetic" }, setup.faux.getModel())).toEqual({
					original: { prompt: "synthetic" },
					id: options!.sessionId,
				});
				return fauxAssistantMessage("Done.");
			}),
		);
		const { harness, root } = await openChat(new MemoryStorage(), setup);
		owners.push(harness);
		const other = await harness.createConversation(
			{ ownership: { kind: "ownerless" }, agent: { model: { provider: "faux", modelId: "faux-1" } } },
			context,
		);
		const [first] = await Promise.all([
			root.submit({ type: "input", content: "Root" }, context),
			other.submit({ type: "input", content: "Other" }, context),
		]);
		const receipt = await first.wait(context);
		if (receipt.status !== "done" || receipt.type !== "input") throw new Error("Expected root answer");
		await other.waitForIdle(context);
		const fork = await root.fork(receipt.answer, { ownership: { kind: "ownerless" } }, context);
		expect((await (await fork.submit({ type: "input", content: "Fork" }, context)).wait(context)).status).toBe(
			"done",
		);
		expect(requests.map((options) => options.sessionId).sort()).toEqual(
			[root.id, other.id, fork.id].map((id) => `conversation:${id}`).sort(),
		);
		for (const options of requests)
			expect(options.headers).toEqual({ shared: "base", conversation: options.sessionId });
		expect(payloads.sort()).toEqual(requests.map((options) => options.sessionId!).sort());
		expect(await harness.snapshot(AgentDoc, root.id, context)).toEqual({
			model: { provider: "faux", modelId: "faux-1" },
		});
	});

	it("rebuilds decorations after retry and restart without persisting callbacks or replacing the Harness signal", async () => {
		const directory = await mkdtemp(join(tmpdir(), "pi-request-options-"));
		directories.push(directory);
		const file = join(directory, "session.sqlite");
		const setup = chatSetup();
		setup.settings.stream = { timeoutMs: 1234, headers: { shared: "base" } };
		setup.settings.retry = { enabled: true, maxRetries: 1, baseDelayMs: 1 };
		const busy = unanswered();
		let calls = 0;
		const signals: AbortSignal[] = [];
		const foreign = new AbortController().signal;
		addHooks(setup.registry, GenerationTask, {
			beforeRequest: (request, api) => {
				expect(request.options.headers!.shared).toBe("base");
				request.options.headers!.shared = "ephemeral";
				calls++;
				signals.push(request.options.signal!);
				const options = {
					sessionId: `conversation:${api.conversationId}:attempt:${calls}`,
					onPayload: () => undefined,
					signal: foreign,
				};
				return { options };
			},
		});
		const sent: string[] = [];
		setup.faux.setResponses([
			(transcript, options, state, model) => {
				sent.push(options!.sessionId!);
				expect(options!.signal).toBe(signals.at(-1));
				return typeof busy.step === "function" ? busy.step(transcript, options, state, model) : busy.step;
			},
			(_transcript, options) => {
				sent.push(options!.sessionId!);
				expect(options!.timeoutMs).toBe(1234);
				expect(options!.signal).toBe(signals.at(-1));
				return fauxAssistantMessage("", { stopReason: "error", errorMessage: "503 Service Unavailable" });
			},
			(_transcript, options) => {
				sent.push(options!.sessionId!);
				expect(options!.signal).toBe(signals.at(-1));
				expect(options!.onPayload).toBeTypeOf("function");
				return fauxAssistantMessage("Recovered.");
			},
		]);
		let opened = await openChat(await openNodeSqliteStorage(file), setup);
		const input = await opened.root.submit({ type: "input", content: "Resume" }, context);
		await busy.reached;
		const tasks = await opened.harness.inspect(context);
		const task = tasks.tasks.find((item) => item.record.kind === "pi.generation")!;
		expect(task.record.state).toMatchObject({
			checkpoint: { phase: "request", streamOptions: { timeoutMs: 1234, headers: { shared: "base" } } },
		});
		expect(JSON.stringify(task.record.state)).not.toContain("sessionId");
		await opened.harness.close(context);
		opened = await openChat(await openNodeSqliteStorage(file), setup);
		owners.push(opened.harness);
		const receipt = await (await opened.harness.submission(input.id, context))!.wait(context);
		expect(receipt.status).toBe("done");
		expect(sent).toEqual([1, 2, 3].map((attempt) => `conversation:${opened.root.id}:attempt:${attempt}`));
		expect(signals.every((signal) => signal !== foreign)).toBe(true);
	});

	it("uses the final hook output cap for the normalized request budget", async () => {
		const setup = chatSetup({ models: [{ id: "faux-1", contextWindow: 22_768, maxTokens: 16_384 }] });
		setup.settings.minimumAnswerTokens = 1_024;
		const sent: SimpleStreamOptions[] = [];
		addHooks(setup.registry, GenerationTask, {
			beforeRequest: () => ({
				messages: [{ role: "user", content: "u".repeat(74_544), timestamp: 1 }],
				options: { maxTokens: 32 },
			}),
		});
		setup.faux.setResponses([
			(_transcript, options) => {
				sent.push(options!);
				return fauxAssistantMessage("Short.");
			},
		]);
		const { harness, root } = await openChat(new MemoryStorage(), setup);
		owners.push(harness);
		expect(
			(await (await root.submit({ type: "input", content: "Short request" }, context)).wait(context)).status,
		).toBe("done");
		expect(sent[0]!.maxTokens).toBe(32);
	});
});
