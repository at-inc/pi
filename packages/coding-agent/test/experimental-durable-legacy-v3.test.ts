import { createModels, fauxAssistantMessage, fauxToolCall, type Message, type Usage } from "@at-inc/pi-ai";
import { BACKGROUND_CONTEXT } from "@earendil-works/chord/context";
import {
	AgentDoc,
	type ConversationId,
	createRegistry,
	type EntryId,
	Harness,
	MemoryStorage,
	ROOT_CONVERSATION_ID,
	UsageDoc,
} from "@earendil-works/pi-durable";
import { afterEach, describe, expect, test } from "vitest";
import { convertToLlm } from "../src/core/messages.ts";
import { buildSessionContext, SessionManager } from "../src/core/session-manager.ts";
import { importLegacyV3, parseLegacyV3 } from "../src/experimental/durable/legacy-v3.ts";

const context = BACKGROUND_CONTEXT;
const timestamp = "2026-09-01T00:00:00.000Z";
const header = { type: "session", version: 3, id: "original-session", timestamp, cwd: "/original" };
const harnesses: Harness[] = [];

afterEach(async () => {
	await Promise.all(harnesses.splice(0).map((harness) => harness.close(context)));
});

function record(id: string, parentId: string | null, fields: Record<string, unknown>): Record<string, unknown> {
	return { id, parentId, timestamp, ...fields };
}

function user(id: string, parentId: string | null, text = id): Record<string, unknown> {
	return record(id, parentId, { type: "message", message: { role: "user", content: text, timestamp: 1 } });
}

function assistant(id: string, parentId: string | null, fields: Record<string, unknown> = {}): Record<string, unknown> {
	return record(id, parentId, {
		type: "message",
		message: { ...fauxAssistantMessage(id, { timestamp: 2 }), ...fields },
	});
}

function bytes(entries: readonly unknown[], sessionHeader: unknown = header): string {
	return [sessionHeader, ...entries].map((entry) => JSON.stringify(entry)).join("\n");
}

function spent(amount: number): Usage {
	return {
		input: amount,
		output: amount,
		cacheRead: amount,
		cacheWrite: amount,
		cacheWrite1h: amount,
		reasoning: amount,
		totalTokens: amount * 4,
		cost: { input: amount, output: amount, cacheRead: amount, cacheWrite: amount, total: amount * 4 },
	};
}

async function imported(entries: readonly unknown[]) {
	const session = parseLegacyV3(bytes(entries));
	const harness = await Harness.open(
		new MemoryStorage(),
		{ models: createModels(), registry: createRegistry() },
		context,
	);
	harnesses.push(harness);
	const result = await importLegacyV3(session, harness, "/target");
	expect(await harness.inspect(context)).toMatchObject({ scheduling: "paused", tasks: [], submissions: [] });
	expect(await harness.commit((tx) => tx.scanTasks({}, 100), context)).toMatchObject({ items: [] });
	expect(JSON.parse(JSON.stringify(result))).toEqual(result);
	return { session, harness, result };
}

describe("strict legacy v3 parsing", () => {
	test("accepts complete final JSON without a newline and preserves header metadata", () => {
		const source = bytes([user("u", null)], { ...header, extra: { archived: true } });
		for (const ending of ["", "\n", "\r\n"]) {
			const parsed = parseLegacyV3(source.replaceAll("\n", "\r\n") + ending);
			expect(parsed.header).toEqual({ ...header, extra: { archived: true } });
			expect(parsed.entries).toEqual([user("u", null)]);
			expect(parsed.warnings).toEqual([]);
		}
	});

	test.each([
		["duplicate ids", [user("u", null), user("u", "u")], /Duplicate/],
		["empty id", [user("", null)], /nonempty/],
		["missing parent", [user("u", "missing")], /Parent/],
		["future parent", [user("u", "future"), user("future", null)], /Parent/],
		["self parent", [user("u", "u")], /Parent/],
		["missing parent field", [{ ...user("u", null), parentId: undefined }], /Parent/],
		["bad timestamp", [{ ...user("u", null), timestamp: "yesterday" }], /timestamp/],
		["invalid calendar date", [{ ...user("u", null), timestamp: "2026-02-30T00:00:00.000Z" }], /calendar/],
		["null message", [record("u", null, { type: "message", message: null })], /message/],
		["missing content", [record("u", null, { type: "message", message: { role: "user", timestamp: 1 } })], /content/],
		[
			"malformed content block",
			[
				record("u", null, {
					type: "message",
					message: { role: "user", content: [{ type: "text", text: 7 }], timestamp: 1 },
				}),
			],
			/text/,
		],
		[
			"unsupported role",
			[record("u", null, { type: "message", message: { role: "extensionTask", content: [], timestamp: 1 } })],
			/Unsupported message role.*stable CLI/,
		],
		["pending assistant", [assistant("a", null, { stopReason: "pending" })], /Pending assistant/],
		["bad stop reason", [assistant("a", null, { stopReason: "unknown" })], /stopReason/],
		["malformed usage", [assistant("a", null, { usage: {} })], /usage/],
		[
			"bad tool call arguments",
			[assistant("a", null, { content: [{ type: "toolCall", id: "c", name: "read", arguments: [] }] })],
			/arguments/,
		],
		["empty model", [record("m", null, { type: "model_change", provider: "faux", modelId: "" })], /modelId/],
		[
			"unsupported thinking",
			[record("t", null, { type: "thinking_level_change", thinkingLevel: "infinite" })],
			/thinkingLevel/,
		],
		[
			"missing label target",
			[record("l", null, { type: "label", targetId: "absent", label: "saved" })],
			/Label target/,
		],
		[
			"missing summary reference",
			[record("s", null, { type: "branch_summary", fromId: "absent", summary: "saved" })],
			/fromId/,
		],
		[
			"missing compaction reference",
			[record("c", null, { type: "compaction", firstKeptEntryId: "absent", summary: "saved", tokensBefore: 1 })],
			/firstKeptEntryId/,
		],
		[
			"compaction on a sibling",
			[
				user("left", null),
				user("right", null),
				record("c", "right", { type: "compaction", firstKeptEntryId: "left", summary: "saved", tokensBefore: 1 }),
			],
			/ancestor/,
		],
		[
			"edit on a sibling",
			[
				user("left", null),
				user("right", null),
				record("e", "right", { type: "context_edit", targetId: "left", replacement: null }),
			],
			/ancestor/,
		],
		[
			"edit of metadata",
			[
				record("m", null, { type: "session_info", name: "name" }),
				record("e", "m", { type: "context_edit", targetId: "m", replacement: null }),
			],
			/editable/,
		],
		[
			"missing replacement",
			[user("u", null), record("e", "u", { type: "context_edit", targetId: "u" })],
			/replacement/,
		],
		[
			"tool content in user edit",
			[
				user("u", null),
				record("e", "u", {
					type: "context_edit",
					targetId: "u",
					replacement: { content: [fauxToolCall("read", {}, { id: "call" })] },
				}),
			],
			/cannot contain tool calls/,
		],
	] as const)("rejects %s", (_name, entries, error) => {
		expect(() => parseLegacyV3(bytes(entries))).toThrow(error);
	});

	test.each([undefined, 1, 2, 4, "3"])("rejects header version %s", (version) => {
		expect(() => parseLegacyV3(bytes([], { ...header, version }))).toThrow(/version exactly 3/);
	});

	test("rejects blank lines, repeated headers, malformed middle records and corrupted tails", () => {
		const good = bytes([user("u", null)]);
		for (const source of [
			"",
			`${good}\n\n`,
			`${good}\n{"type":`,
			`${JSON.stringify(header)}\nnot-json\n${JSON.stringify(user("u", null))}`,
			bytes([header]),
		]) {
			expect(() => parseLegacyV3(source)).toThrow(/Invalid v3 session/);
		}
	});
});

test("imports normal messages and custom contributions with exact raw records, without replay", async () => {
	const entries = [
		record("s", null, {
			type: "message",
			message: { role: "system", content: "System", sections: { rule: "Rule" }, timestamp: 0 },
		}),
		user("u", "s"),
		assistant("a", "u", {
			content: [
				{ type: "thinking", thinking: "Think", thinkingSignature: "opaque" },
				{ type: "text", text: "Answer", textSignature: "sig" },
			],
		}),
		record("custom", "a", {
			type: "custom_message",
			customType: "annotation",
			content: "custom text",
			display: true,
			details: { privateState: true },
		}),
		record("bash", "custom", {
			type: "message",
			message: {
				role: "bashExecution",
				command: "echo old",
				output: "old",
				exitCode: 0,
				cancelled: false,
				truncated: false,
				timestamp: 3,
			},
		}),
		record("hidden", "bash", {
			type: "message",
			message: {
				role: "bashExecution",
				command: "echo excluded",
				output: "excluded",
				exitCode: 0,
				cancelled: false,
				truncated: false,
				excludeFromContext: true,
				timestamp: 4,
			},
		}),
		record("state", "hidden", { type: "custom", customType: "extension-state", data: { retained: [1, 2] } }),
		record("tools", "state", { type: "active_tools_change", toolNames: ["old-extension-tool"] }),
		record("name", "tools", { type: "session_info", name: "Original name" }),
	];
	const { session, harness, result } = await imported(entries);
	const root = await harness.root(context);
	expect((await root.context(context)).messages).toEqual(
		convertToLlm(buildSessionContext([...session.entries]).messages),
	);
	expect((await root.entries({}, 100, undefined, context)).items).toHaveLength(entries.length + 1);
	for (const original of entries) {
		const saved = await harness.commit((tx) => tx.entry(result.entryIds[original.id as string] as EntryId), context);
		expect(saved?.data).toEqual(original);
	}
	expect(result.warnings.join("\n")).toMatch(/archive-only/);
	expect(result.warnings.join("\n")).toContain("active_tools_change");
	expect(await harness.snapshot(AgentDoc, root.id, context)).toEqual({
		cwd: "/target",
		thinkingLevel: "off",
		model: { provider: "faux", modelId: "faux-1" },
	});
});

test("places the last physical path on root and forks both shared prefixes and separate roots once", async () => {
	const entries = [
		record("m", null, { type: "model_change", provider: "faux", modelId: "initial" }),
		record("thinking", "m", { type: "thinking_level_change", thinkingLevel: "high" }),
		user("shared", "thinking"),
		assistant("common", "shared", { model: "shared-model", usage: spent(1) }),
		assistant("left", "common", { model: "left-model", usage: spent(2) }),
		user("other-root", null),
		assistant("other", "other-root", { model: "other-model", usage: spent(4) }),
		record("label", "other", { type: "label", targetId: "left", label: "Bookmarked branch" }),
		record("low", "common", { type: "thinking_level_change", thinkingLevel: "low" }),
		assistant("active", "low", { model: "active-model", usage: spent(8) }),
	];
	const { session, harness, result } = await imported(entries);
	expect(result.branches).toHaveLength(3);
	expect(result.branches.find((branch) => branch.leafId === "active")?.conversationId).toBe(ROOT_CONVERSATION_ID);
	expect(result.branches.find((branch) => branch.leafId === "left")?.label).toBe("Bookmarked branch");
	expect(Object.keys(result.entryIds)).toHaveLength(entries.length);
	expect(new Set(Object.values(result.entryIds)).size).toBe(entries.length);
	const seen = new Set<number>();
	for (const branch of result.branches) {
		const id = branch.conversationId as ConversationId;
		const conversation = (await harness.conversation(id, context))!;
		const expected = buildSessionContext([...session.entries], branch.leafId);
		expect((await conversation.context(context)).messages).toEqual(convertToLlm(expected.messages));
		expect(await harness.snapshot(AgentDoc, id, context)).toEqual({
			model: expected.model,
			thinkingLevel: expected.thinkingLevel,
			cwd: "/target",
		});
		for (const entry of (await conversation.entries({}, 100, undefined, context)).items) seen.add(entry.id);
	}
	expect(seen.size).toBe(entries.length + 1);
	const rootUsage = await harness.snapshot(UsageDoc, ROOT_CONVERSATION_ID, context);
	expect(rootUsage?.models).toEqual({ "faux/shared-model": spent(1), "faux/active-model": spent(8) });
	expect((await harness.usage(context)).models).toEqual({
		"faux/shared-model": spent(1),
		"faux/left-model": spent(2),
		"faux/other-model": spent(4),
		"faux/active-model": spent(8),
	});
});

test("preserves branch settings at a fork before later model and thinking changes", async () => {
	const { harness, result } = await imported([
		record("m", null, { type: "model_change", provider: "faux", modelId: "first" }),
		record("t", "m", { type: "thinking_level_change", thinkingLevel: "high" }),
		user("u", "t"),
		record("m2", "u", { type: "model_change", provider: "faux", modelId: "second" }),
		record("t2", "m2", { type: "thinking_level_change", thinkingLevel: "low" }),
		user("active", "t2"),
		user("early-fork", "u"),
	]);
	for (const branch of result.branches) {
		expect(await harness.snapshot(AgentDoc, branch.conversationId as ConversationId, context)).toEqual({
			cwd: "/target",
			thinkingLevel: branch.leafId === "active" ? "low" : "high",
			model: { provider: "faux", modelId: branch.leafId === "active" ? "second" : "first" },
		});
	}
});

test("preserves repeated compactions, retained edits, system checkpoints, and a self head", async () => {
	const checkpoint: Message = {
		role: "system",
		content: "Checkpoint",
		sections: { rule: "Saved rule" },
		timestamp: 5,
	};
	const entries = [
		user("old", null),
		user("kept", "old"),
		record("system", "kept", {
			type: "message",
			message: { role: "system", content: "Pre-compaction", timestamp: 1 },
		}),
		record("e", "system", { type: "context_edit", targetId: "kept", replacement: { content: "edited kept" } }),
		record("c1", "e", {
			type: "compaction",
			summary: "First summary",
			firstKeptEntryId: "kept",
			tokensBefore: 10,
			systemMessage: checkpoint,
		}),
		user("tail", "c1"),
		record("c2", "tail", {
			type: "compaction",
			summary: "Second summary",
			firstKeptEntryId: "kept",
			tokensBefore: 20,
			systemMessage: { ...checkpoint, timestamp: 6 },
		}),
		user("after", "c2"),
		record("c3", "c2", {
			type: "compaction",
			summary: "No retained tail",
			firstKeptEntryId: "c3",
			tokensBefore: 30,
			systemMessage: { ...checkpoint, timestamp: 7 },
		}),
	];
	const { session, harness, result } = await imported(entries);
	for (const branch of result.branches) {
		const conversation = (await harness.conversation(branch.conversationId as ConversationId, context))!;
		const messages = (await conversation.context(context)).messages;
		expect(messages).toEqual(convertToLlm(buildSessionContext([...session.entries], branch.leafId).messages));
		expect(JSON.stringify(messages)).not.toContain("First summary");
		expect(JSON.stringify(messages)).not.toContain("Pre-compaction");
	}
	const c2 = await harness.commit((tx) => tx.entry(result.entryIds.c2 as EntryId), context);
	expect(c2?.head).toBe(result.entryIds.kept);
	expect(c2?.edits).toContainEqual({ target: result.entryIds.system, action: "omit" });
	const c3 = await harness.commit((tx) => tx.entry(result.entryIds.c3 as EntryId), context);
	expect(c3?.head).toBe(result.entryIds.c3);
	expect(
		SessionManager.inMemory("/original", undefined, [session.header, ...session.entries]).buildSessionContext()
			.messages,
	).toHaveLength(2);
});

test("maps edits to user, assistant, tool and custom content and keeps them branch-local", async () => {
	const call = fauxToolCall("read", { path: "old" }, { id: "original-call-id" });
	const entries = [
		user("u", null),
		assistant("a", "u", { content: [call], stopReason: "toolUse" }),
		record("r", "a", {
			type: "message",
			message: {
				role: "toolResult",
				toolCallId: call.id,
				toolName: "read",
				content: [{ type: "text", text: "old output" }],
				isError: false,
				timestamp: 3,
			},
		}),
		record("custom", "r", { type: "custom_message", customType: "note", content: "old custom", display: false }),
		record("eu", "custom", {
			type: "context_edit",
			targetId: "u",
			replacement: { content: [{ type: "text", text: "edited user" }] },
		}),
		record("ea", "eu", {
			type: "context_edit",
			targetId: "a",
			replacement: { content: [{ type: "text", text: "edited assistant" }, call] },
		}),
		record("er", "ea", { type: "context_edit", targetId: "r", replacement: { content: "edited result" } }),
		record("ec", "er", { type: "context_edit", targetId: "custom", replacement: { content: "edited custom" } }),
		record("omit", "ec", { type: "context_edit", targetId: "u", replacement: null }),
		user("unedited", "custom"),
	];
	const { session, harness, result } = await imported(entries);
	for (const branch of result.branches) {
		const conversation = (await harness.conversation(branch.conversationId as ConversationId, context))!;
		expect((await conversation.context(context)).messages).toEqual(
			convertToLlm(buildSessionContext([...session.entries], branch.leafId).messages),
		);
	}
	expect((await harness.commit((tx) => tx.entry(result.entryIds.er as EntryId), context))?.edits).toEqual([
		{
			target: result.entryIds.r,
			action: "replace",
			messages: [
				{
					role: "toolResult",
					toolCallId: call.id,
					toolName: "read",
					content: [{ type: "text", text: "edited result" }],
					isError: false,
					timestamp: 3,
				},
			],
		},
	]);
	expect((await harness.commit((tx) => tx.entry(result.entryIds.omit as EntryId), context))?.edits).toEqual([
		{ target: result.entryIds.u, action: "omit" },
	]);
});

test("normalizes tool order, missing results and partial assistants without replaying calls", async () => {
	const calls = ["one", "two", "missing"].map((id) => fauxToolCall("read", {}, { id }));
	const entries = [
		user("u", null),
		assistant("a", "u", { content: calls, stopReason: "toolUse" }),
		user("interleaved", "a"),
		...(["two", "one", "orphan"] as const).map((id, i) =>
			record(id, i === 0 ? "interleaved" : ["two", "one"][i - 1]!, {
				type: "message",
				message: {
					role: "toolResult",
					toolCallId: id,
					toolName: "read",
					content: [{ type: "text", text: id }],
					isError: false,
					timestamp: i + 3,
				},
			}),
		),
		assistant("aborted", "orphan", { stopReason: "aborted", usage: spent(1) }),
		assistant("error", "aborted", { stopReason: "error", usage: spent(2) }),
		assistant("deferred", "error", {
			stopReason: "deferred",
			usage: spent(4),
			deferred: { provider: "faux", modelId: "faux-1", api: "faux", id: "old-handle" },
		}),
	];
	const { harness, result } = await imported(entries);
	const root = await harness.root(context);
	const messages = (await root.context(context)).messages;
	expect(messages.map((message) => message.role)).toEqual([
		"user",
		"assistant",
		"toolResult",
		"toolResult",
		"toolResult",
		"user",
	]);
	expect(messages.filter((message) => message.role === "toolResult").map((message) => message.toolCallId)).toEqual([
		"one",
		"two",
		"missing",
	]);
	expect(messages[4]).toMatchObject({ isError: true, details: { reason: "missing_result" } });
	for (const marker of ["missing", "orphan", "aborted", "error", "deferred"])
		expect(result.warnings.join("\n")).toContain(marker);
	expect((await harness.usage(context)).models["faux/faux-1"]).toEqual(spent(7));
	for (const id of ["aborted", "error", "deferred"])
		expect((await harness.commit((tx) => tx.entry(result.entryIds[id] as EntryId), context))?.data).toEqual(
			entries.find((entry) => entry.id === id),
		);
});

test("preserves summary, custom and tool usage without charging shared prefixes twice", async () => {
	const call = fauxToolCall("__proto__", {}, { id: "call" });
	const entries = [
		assistant("__proto__", null, { usage: spent(1), content: [call], stopReason: "toolUse" }),
		record("r", "__proto__", {
			type: "message",
			message: {
				role: "toolResult",
				toolCallId: "call",
				toolName: "__proto__",
				content: [],
				isError: false,
				timestamp: 3,
				usage: spent(2),
			},
		}),
		record("usage", "r", {
			type: "usage",
			kind: "cache_warm",
			provider: "faux",
			model: "faux-1",
			usage: spent(4),
			note: "original qualifier",
		}),
		record("c", "usage", {
			type: "compaction",
			firstKeptEntryId: "c",
			summary: "Summary",
			tokensBefore: 12,
			usage: spent(8),
		}),
		record("b", "usage", {
			type: "branch_summary",
			fromId: "c",
			summary: "Branch summary",
			usage: spent(16),
			details: { original: true },
		}),
	];
	const { harness, result } = await imported(entries);
	expect(Object.hasOwn(result.entryIds, "__proto__")).toBe(true);
	expect((await harness.usage(context)).models).toEqual({ "faux/faux-1": spent(29) });
	const tools = (await harness.usage(context)).tools;
	expect(Object.hasOwn(tools, "__proto__")).toBe(true);
	expect(tools.__proto__).toEqual(spent(2));
	expect((await harness.commit((tx) => tx.entry(result.entryIds.b as EntryId), context))?.kind).toBe("pi.user");
});

test("imports an empty session and refuses to append a second import", async () => {
	const { harness, session, result } = await imported([]);
	expect(result.entryIds).toEqual({});
	expect(result.branches).toEqual([]);
	expect((await (await harness.root(context)).context(context)).messages).toEqual([]);
	await expect(importLegacyV3(session, harness, "/target")).rejects.toThrow(/brand-new empty Harness/);
});

test("rejects string-coercible state fields and whitespace-only tail records", () => {
	for (const entry of [
		assistant("a", null, { stopReason: ["stop"] }),
		assistant("a", null, { thinkingLevel: ["high"] }),
		record("t", null, { type: "thinking_level_change", thinkingLevel: ["off"] }),
		user(header.id, null),
	])
		expect(() => parseLegacyV3(bytes([entry]))).toThrow(/Invalid v3 session/);
	expect(() => parseLegacyV3(`${bytes([user("u", null)])}\n   `)).toThrow(/Blank lines/);
});

test("warns when identical duplicate tool results are omitted from context", async () => {
	const result = {
		role: "toolResult",
		toolCallId: "call",
		toolName: "read",
		content: [],
		isError: false,
		timestamp: 3,
	};
	const { harness, result: report } = await imported([
		assistant("a", null, { content: [fauxToolCall("read", {}, { id: "call" })], stopReason: "toolUse" }),
		record("r1", "a", { type: "message", message: result }),
		record("r2", "r1", { type: "message", message: result }),
	]);
	expect((await (await harness.root(context)).context(context)).messages).toHaveLength(2);
	expect(report.warnings.join("\n")).toMatch(/duplicate tool result call/);
});
