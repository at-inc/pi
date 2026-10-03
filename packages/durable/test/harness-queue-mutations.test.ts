import { fauxAssistantMessage, fauxToolCall, Type } from "@at-inc/pi-ai";
import {
	defineTool,
	type Harness,
	InboxDoc,
	type PreparedInputEntry,
	type SubmissionId,
} from "@earendil-works/pi-durable";
import { afterEach, describe, expect, it } from "vitest";
import { allEntries, chatSetup, openChat, unanswered } from "./chat-support.ts";
import { addTool } from "./harness-support.ts";
import { ControlledStorage, context } from "./session-support.ts";

const owners: Harness[] = [];
const releases: (() => void)[] = [];

afterEach(async () => {
	for (const release of releases.splice(0)) release();
	for (const owner of owners.splice(0)) await owner.close(context);
});

async function busy(at: "postTools" | "final", holdNext = false) {
	const setup = chatSetup();
	setup.settings.followUpMode = "all";
	setup.settings.steeringMode = "all";
	const reached = Promise.withResolvers<void>();
	const release = Promise.withResolvers<void>();
	releases.push(release.resolve);
	const next = unanswered();
	if (at === "postTools") {
		addTool(
			setup.registry,
			defineTool({
				name: "gate",
				description: "Gate",
				parameters: Type.Object({}),
				execute: async () => {
					reached.resolve();
					await release.promise;
					return { content: [] };
				},
			}),
		);
		setup.faux.setResponses([
			fauxAssistantMessage([fauxToolCall("gate", {}, { id: "call-gate" })], { stopReason: "toolUse" }),
			holdNext ? next.step : fauxAssistantMessage("Queue answered."),
		]);
	} else {
		setup.faux.setResponses([
			async () => {
				reached.resolve();
				await release.promise;
				return fauxAssistantMessage("First answered.");
			},
			holdNext ? next.step : fauxAssistantMessage("Queue answered."),
		]);
	}
	const storage = new ControlledStorage();
	const opened = await openChat(storage, setup);
	owners.push(opened.harness);
	const first = await opened.root.submit({ type: "input", content: "Busy" }, context);
	await reached.promise;
	return {
		...opened,
		setup,
		storage,
		first,
		release,
		next,
		whenBusy: at === "postTools" ? ("steer" as const) : ("followUp" as const),
	};
}

describe.each(["postTools", "final"] as const)("queued input mutation at %s", (at) => {
	it("promotes the same prepared receipt to the front of steering without changing a pending wake", async () => {
		const { root, harness, setup, first, release } = await busy(at);
		setup.faux.appendResponses([fauxAssistantMessage("Wake answered.")]);
		const earlier = await root.submit({ type: "input", content: "Earlier steer", whenBusy: "steer" }, context);
		const input = await root.submit(
			{
				type: "input",
				content: "Prepared",
				requestId: "promote",
				identity: "prepared-command",
				entry: {
					kind: "app.promoted-input",
					model: [{ role: "user", content: "Prepared", timestamp: 1 }],
					data: { attachment: "preserved" },
				},
			},
			context,
		);
		const wake = await root.wake({ requestId: "pending-wake" }, context);
		const original = Object.freeze(await input.status(context));
		const beforeWake = await wake.status(context);
		await root.promoteQueuedInput(input.id, context);
		const items = (await harness.snapshot(InboxDoc, root.id, context))!.items;
		expect(items.map((item) => item.id)).toEqual([input.id, earlier.id, wake.id]);
		expect(items[0]).toMatchObject({
			mode: "steer",
			entry: { kind: "app.promoted-input", data: { attachment: "preserved" } },
		});
		expect(items[2]).toEqual({ id: wake.id, mode: "followUp", wake: true });
		expect(await input.status(context)).toEqual(original);
		expect(await wake.status(context)).toEqual(beforeWake);
		await expect(root.promoteQueuedInput(wake.id, context)).rejects.toThrow("is not queued");
		release.resolve();
		await first.wait(context);
		const receipt = await input.wait(context);
		expect(receipt).toMatchObject({ status: "done", requestId: "promote", identity: "prepared-command" });
		await earlier.wait(context);
		await wake.wait(context);
		expect((await allEntries(root)).find((entry) => entry.id === receipt.entry)).toMatchObject({
			kind: "app.promoted-input",
			data: { attachment: "preserved" },
		});
		await expect(root.promoteQueuedInput(input.id, context)).rejects.toThrow("is not queued");
	});
	it("edits and moves the native inbox without changing receipt identity or mutating prepared input data", async () => {
		const { root, harness, storage, first, release, whenBusy } = await busy(at);
		const a = await root.submit({ type: "input", content: "A", requestId: "a", whenBusy }, context);
		const b = await root.submit({ type: "input", content: "B", requestId: "b", whenBusy }, context);
		const frozen = Object.freeze(await a.status(context));
		const entry = {
			kind: "app.edited-input",
			model: [{ role: "user", content: "Edited A", timestamp: 1 }] as const,
			data: { display: { text: "Edited A" } },
		};
		await root.editQueuedInput(a.id, { content: "Edited A", entry }, context);
		entry.data.display.text = "caller mutation";
		await root.moveQueuedInput(b.id, a.id, context);
		expect((await harness.snapshot(InboxDoc, root.id, context))?.items.map((item) => item.id)).toEqual([b.id, a.id]);
		await root.moveQueuedInput(b.id, undefined, context);
		expect((await harness.snapshot(InboxDoc, root.id, context))?.items.map((item) => item.id)).toEqual([a.id, b.id]);
		await root.moveQueuedInput(b.id, a.id, context);
		await root.moveQueuedInput(a.id, a.id, context);
		expect(await a.status(context)).toEqual(frozen);
		expect(Object.isFrozen(frozen)).toBe(true);
		expect(frozen).toMatchObject({ status: "queued", requestId: "a" });
		expect((await allEntries(root)).some((record) => record.kind === "app.edited-input")).toBe(false);
		release.resolve();
		expect((await first.wait(context)).status).toBe("done");
		expect((await a.wait(context)).status).toBe("done");
		expect((await b.wait(context)).status).toBe("done");
		const entries = await allEntries(root);
		const bReceipt = await b.status(context);
		const aReceipt = await a.status(context);
		expect(entries.findIndex((entry) => entry.id === bReceipt.entry)).toBeLessThan(
			entries.findIndex((entry) => entry.id === aReceipt.entry),
		);
		expect(entries.find((entry) => entry.id === aReceipt.entry)).toMatchObject({
			kind: "app.edited-input",
			data: { display: { text: "Edited A" } },
		});
		expect(
			storage.commits.filter((batch) =>
				batch.some(
					(write) => write.type === "submission" && write.value.id === a.id && write.value.status === "queued",
				),
			),
		).toHaveLength(1);
		expect(frozen.status).toBe("queued");
	});

	it("rejects edits and moves once the boundary has placed the input", async () => {
		const { root, first, release, next, whenBusy } = await busy(at, true);
		const queued = await root.submit({ type: "input", content: "Original", whenBusy }, context);
		release.resolve();
		await next.reached;
		expect((await queued.status(context)).status).toBe("placed");
		await expect(root.editQueuedInput(queued.id, { content: "Late" }, context)).rejects.toThrow("is not queued");
		await expect(root.moveQueuedInput(queued.id, undefined, context)).rejects.toThrow("is not queued");
		expect(
			(await root.context(context)).messages.some(
				(message) => message.role === "user" && message.content === "Original",
			),
		).toBe(true);
		await root.abort(context);
		await first.wait(context);
		await queued.wait(context);
	});
});

describe("queued input validation", () => {
	it("rejects foreign, write, withdrawn, missing, and placed IDs without changing the native queue", async () => {
		const { root, harness, storage, first, release } = await busy("final");
		const a = await root.submit({ type: "input", content: "A" }, context);
		const withdrawn = await root.submit({ type: "input", content: "Withdrawn" }, context);
		await withdrawn.abort(context);
		const write = await root.submit({ type: "write", entry: { kind: "app.note" } }, context);
		const other = await harness.createConversation({ ownership: { kind: "ownerless" } }, context);
		const foreign = await other.submit({ type: "write", entry: { kind: "foreign" } }, context);
		const before = await harness.snapshot(InboxDoc, root.id, context);
		for (const id of [write.id, withdrawn.id, foreign.id, first.id, 999_999 as SubmissionId]) {
			const count = storage.commits.length;
			await expect(root.editQueuedInput(id, { content: "Invalid" }, context)).rejects.toThrow("is not queued");
			await expect(root.moveQueuedInput(id, a.id, context)).rejects.toThrow("is not queued");
			await expect(root.moveQueuedInput(a.id, id, context)).rejects.toThrow("is not queued");
			await expect(root.promoteQueuedInput(id, context)).rejects.toThrow("is not queued");
			expect(storage.commits).toHaveLength(count);
		}
		const invalid = { kind: "app.input", head: "self" } as unknown as PreparedInputEntry;
		await expect(
			root.editQueuedInput(a.id, { content: "No context mutation", entry: invalid }, context),
		).rejects.toThrow("cannot set head");
		expect(await harness.snapshot(InboxDoc, root.id, context)).toEqual(before);
		release.resolve();
		await first.wait(context);
		await a.wait(context);
	});

	it("clears a prepared override when an edit supplies only replacement content", async () => {
		const { root, first, release } = await busy("final");
		const input = await root.submit(
			{
				type: "input",
				content: "Old",
				entry: {
					kind: "app.input",
					model: [{ role: "user", content: "Old projection", timestamp: 1 }],
					data: { display: "Old display" },
				},
			},
			context,
		);
		await root.editQueuedInput(input.id, { content: "Replacement" }, context);
		release.resolve();
		await first.wait(context);
		const receipt = await input.wait(context);
		expect((await allEntries(root)).find((record) => record.id === receipt.entry)).toMatchObject({
			kind: "pi.user",
			model: [{ role: "user", content: "Replacement", timestamp: expect.any(Number) }],
		});
	});
});
