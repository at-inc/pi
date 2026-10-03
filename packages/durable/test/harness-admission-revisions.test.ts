import { mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fauxAssistantMessage } from "@at-inc/pi-ai";
import {
	ConversationBusy,
	defineDoc,
	type Harness,
	InboxDoc,
	MemoryStorage,
	type Seq,
	watchEvents,
} from "@earendil-works/pi-durable";
import { afterEach, describe, expect, it } from "vitest";
import { openNodeJsonlStorage } from "../src/storage/jsonl/node.ts";
import { openNodeSqliteStorage } from "../src/storage/sqlite/node.ts";
import { allEntries, chatSetup, openChat, unanswered, waitFor } from "./chat-support.ts";
import { context } from "./session-support.ts";

const owners: Harness[] = [];
const directories: string[] = [];
const releases: (() => void)[] = [];

afterEach(async () => {
	for (const release of releases.splice(0)) release();
	for (const owner of owners.splice(0)) await owner.close(context);
	for (const directory of directories.splice(0)) await rm(directory, { recursive: true, force: true });
});

async function directory() {
	const path = await mkdtemp(join(tmpdir(), "pi-native-admission-"));
	directories.push(path);
	return path;
}

describe("atomic opaque submission identity", () => {
	it("rejects concurrent request identity collisions on the same Session line", async () => {
		const setup = chatSetup();
		setup.faux.setResponses([fauxAssistantMessage("Answer.")]);
		const { harness, root } = await openChat(new MemoryStorage(), setup);
		owners.push(harness);
		const results = await Promise.allSettled([
			root.submit({ type: "input", requestId: "same", identity: "payload-a", content: "A" }, context),
			root.submit({ type: "input", requestId: "same", identity: "payload-b", content: "B" }, context),
		]);
		expect(results.filter((result) => result.status === "fulfilled")).toHaveLength(1);
		expect(results.filter((result) => result.status === "rejected")).toHaveLength(1);
		const first = results[0];
		if (first?.status !== "fulfilled") throw new Error("Expected the first ordered admission");
		expect(await first.value.wait(context)).toMatchObject({ status: "done", identity: "payload-a" });
		expect((await allEntries(root)).filter((entry) => entry.kind === "pi.user")).toHaveLength(1);
		await expect(root.submit({ type: "input", requestId: "same", content: "A" }, context)).rejects.toThrow(
			"different submission identity",
		);
	});

	it("preserves identity through queued, delivered, terminal, and reopened receipts", async () => {
		const file = join(await directory(), "session.sqlite");
		const setup = chatSetup();
		const started = Promise.withResolvers<void>();
		const release = Promise.withResolvers<void>();
		releases.push(release.resolve);
		setup.faux.setResponses([
			async () => {
				started.resolve();
				await release.promise;
				return fauxAssistantMessage("First.");
			},
			fauxAssistantMessage("Second."),
		]);
		let opened = await openChat(await openNodeSqliteStorage(file), setup);
		const first = await opened.root.submit({ type: "input", content: "Busy" }, context);
		await started.promise;
		const queued = await opened.root.submit(
			{ type: "input", content: "Queued", requestId: "strict", identity: "full-payload" },
			context,
		);
		expect(await queued.status(context)).toMatchObject({ status: "queued", identity: "full-payload" });
		await expect(
			opened.root.submit(
				{ type: "input", content: "Changed", requestId: "strict", identity: "changed-payload" },
				context,
			),
		).rejects.toThrow("different submission identity");
		release.resolve();
		await first.wait(context);
		const receipt = await queued.wait(context);
		expect(receipt).toMatchObject({ status: "done", identity: "full-payload" });
		await opened.harness.close(context);
		opened = await openChat(await openNodeSqliteStorage(file), setup);
		owners.push(opened.harness);
		const reused = await opened.root.submit(
			{ type: "input", content: "Reused", requestId: "strict", identity: "full-payload" },
			context,
		);
		expect(reused.id).toBe(queued.id);
		expect(await reused.wait(context)).toEqual(receipt);
		await expect(
			opened.root.submit(
				{ type: "write", entry: { kind: "note" }, requestId: "strict", identity: "full-payload" },
				context,
			),
		).rejects.toThrow("type input");
		await expect(
			opened.root.submit({ type: "input", content: "Changed", requestId: "strict", identity: "different" }, context),
		).rejects.toThrow("different submission identity");
	});
});

describe("explicit native wake", () => {
	it("continues after an interrupt from existing context without a generated user row or hidden prompt", async () => {
		const setup = chatSetup();
		const busy = unanswered();
		setup.faux.setResponses([busy.step]);
		const { harness, root } = await openChat(new MemoryStorage(), setup);
		owners.push(harness);
		const interrupted = await root.submit({ type: "input", content: "Original task" }, context);
		await busy.reached;
		await root.abort(context);
		expect((await interrupted.wait(context)).status).toBe("unanswered");
		const history = await allEntries(root);
		const transcript = await root.context(context);
		setup.faux.setResponses([
			async (request) => {
				expect(await allEntries(root)).toEqual(history);
				expect(request.messages).toEqual(transcript.messages);
				return fauxAssistantMessage("Continued.");
			},
		]);
		const waking = await root.wake({ requestId: "continue", identity: "continue-original" }, context);
		const receipt = await waking.wait(context);
		expect(receipt).toMatchObject({
			type: "wake",
			status: "done",
			identity: "continue-original",
			answer: expect.any(Number),
		});
		expect(receipt.entry).toBeUndefined();
		expect((await allEntries(root)).filter((entry) => entry.kind === "pi.user")).toHaveLength(1);
		const reused = await root.wake({ requestId: "continue", identity: "continue-original" }, context);
		expect(reused.id).toBe(waking.id);
		expect(setup.faux.state.callCount).toBe(2);
	});

	it("queues one native wake receipt while busy and honors reject and abort", async () => {
		const setup = chatSetup();
		const started = Promise.withResolvers<void>();
		const release = Promise.withResolvers<void>();
		releases.push(release.resolve);
		setup.faux.setResponses([
			async () => {
				started.resolve();
				await release.promise;
				return fauxAssistantMessage("First.");
			},
			fauxAssistantMessage("Wake."),
		]);
		const { harness, root } = await openChat(new MemoryStorage(), setup);
		owners.push(harness);
		const first = await root.submit({ type: "input", content: "Busy" }, context);
		await started.promise;
		await expect(root.wake({ whenBusy: "reject" }, context)).rejects.toBeInstanceOf(ConversationBusy);
		const withdrawn = await root.wake({ requestId: "withdraw" }, context);
		expect(await withdrawn.abort(context)).toBe("aborted");
		const wake = await root.wake({ requestId: "queue", identity: "wake" }, context);
		const reused = await root.wake({ requestId: "queue", identity: "wake" }, context);
		expect(reused.id).toBe(wake.id);
		expect((await harness.snapshot(InboxDoc, root.id, context))?.items).toEqual([
			{ id: wake.id, mode: "followUp", wake: true },
		]);
		release.resolve();
		await first.wait(context);
		expect((await wake.wait(context)).status).toBe("done");
		expect((await allEntries(root)).filter((entry) => entry.kind === "pi.user")).toHaveLength(1);
		expect(setup.faux.state.callCount).toBe(2);
	});

	it("keeps an interrupted wake receipt durable across close and reopen without duplicating generation", async () => {
		const file = join(await directory(), "wake.sqlite");
		const setup = chatSetup();
		const busy = unanswered();
		setup.faux.setResponses([fauxAssistantMessage("First."), busy.step, fauxAssistantMessage("Reopened wake.")]);
		let opened = await openChat(await openNodeSqliteStorage(file), setup);
		await (await opened.root.submit({ type: "input", content: "Original" }, context)).wait(context);
		const wake = await opened.root.wake({ requestId: "persisted", identity: "wake" }, context);
		await busy.reached;
		await opened.harness.close(context);
		opened = await openChat(await openNodeSqliteStorage(file), setup);
		owners.push(opened.harness);
		expect((await (await opened.harness.submission(wake.id, context))!.wait(context)).status).toBe("done");
		expect((await opened.root.wake({ requestId: "persisted", identity: "wake" }, context)).id).toBe(wake.id);
		expect((await allEntries(opened.root)).filter((entry) => entry.kind === "pi.user")).toHaveLength(1);
	});
});

describe("truthful event revisions", () => {
	it("uses adopted publication sequences even when earlier commits emit no event callbacks", async () => {
		class GappedStorage extends MemoryStorage {
			override async commit(writes: Parameters<MemoryStorage["commit"]>[0]): Promise<Seq> {
				return this.prepareCommit(writes, ((await this.currentSeq(context)) + 7) as Seq).apply();
			}
		}
		const storage = new GappedStorage();
		const { harness, root } = await openChat(storage, chatSetup());
		owners.push(harness);
		const hidden = defineDoc<{ n: number }>({
			kind: "test.hidden",
			version: 1,
			scope: "conversation",
			history: "latest",
			fork: "initial",
			initial: () => ({ n: 0 }),
		});
		const stream = await watchEvents(harness, root.id, context);
		expect(stream.snapshotSeq).toBe(await storage.currentSeq(context));
		const adopted: number[] = [];
		harness.subscribeCommits((publication) => adopted.push(publication.seq));
		const delivered: number[] = [];
		stream.start(async (_batch, _context, seq) => {
			delivered.push(seq);
		});
		await root.commit(async (tx) => {
			(await tx.doc(hidden, root.id)).n++;
		}, context);
		await root.commit((tx) => tx.appendEntry(root.id, { kind: "note" }), context);
		await waitFor(() => delivered.length === 1);
		expect(delivered).toEqual([adopted[1]]);
		await stream.stop();
		await root.commit((tx) => tx.appendEntry(root.id, { kind: "while-detached" }), context);
		const next = await watchEvents(harness, root.id, context);
		expect(next.snapshotSeq).toBe(await storage.currentSeq(context));
		await next.stop();
	});

	it("pairs an overflow snapshot with its actual latest sequence", async () => {
		const storage = new MemoryStorage();
		const { harness, root } = await openChat(storage, chatSetup());
		owners.push(harness);
		const stream = await watchEvents(harness, root.id, context);
		for (let index = 0; index < 101; index++)
			await root.commit((tx) => tx.appendEntry(root.id, { kind: "note" }), context);
		const expected = await storage.currentSeq(context);
		let delivered: number | undefined;
		stream.start(async (events, _context, seq) => {
			expect(events[0]?.type).toBe("snapshot");
			delivered = seq;
		});
		await waitFor(() => delivered !== undefined);
		expect(delivered).toBe(expected);
		await stream.stop();
	});
});

describe.each(["memory", "jsonl", "sqlite"] as const)("%s storage high-water sequence", (backend) => {
	it("counts successful commits and preserves snapshot metadata across reopen", async () => {
		const path = backend === "memory" ? undefined : await directory();
		let storage =
			backend === "memory"
				? new MemoryStorage()
				: backend === "jsonl"
					? await openNodeJsonlStorage(path!, context)
					: await openNodeSqliteStorage(join(path!, "storage.sqlite"));
		expect(await storage.currentSeq(context)).toBe(0);
		const setup = chatSetup();
		let opened = await openChat(storage, setup);
		const document = defineDoc<{ n: number }>({
			kind: "test.sequence",
			version: 1,
			scope: "conversation",
			history: "latest",
			fork: "initial",
			initial: () => ({ n: 0 }),
		});
		await opened.root.commit(async (tx) => {
			(await tx.doc(document, opened.root.id)).n = 1;
		}, context);
		const expected = await storage.currentSeq(context);
		const stream = await watchEvents(opened.harness, opened.root.id, context);
		expect(stream.snapshotSeq).toBe(expected);
		await stream.stop();
		await opened.harness.close(context);
		await expect(storage.currentSeq(context)).rejects.toThrow("closed");
		if (backend !== "memory") {
			storage =
				backend === "jsonl"
					? await openNodeJsonlStorage(path!, context)
					: await openNodeSqliteStorage(join(path!, "storage.sqlite"));
			expect(await storage.currentSeq(context)).toBe(expected);
			opened = await openChat(storage, setup);
			const reopened = await watchEvents(opened.harness, opened.root.id, context);
			expect(reopened.snapshotSeq).toBe(expected);
			await reopened.stop();
			await opened.harness.close(context);
		}
	});
});
