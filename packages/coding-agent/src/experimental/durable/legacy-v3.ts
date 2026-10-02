import { isDeepStrictEqual } from "node:util";
import { createModels, type ModelThinkingLevel, type Usage } from "@at-inc/pi-ai";
import { copyJson, type JsonRepresentation } from "@earendil-works/chord";
import { BACKGROUND_CONTEXT } from "@earendil-works/chord/context";
import {
	AgentDoc,
	type ContextEdit,
	type ConversationId,
	createRegistry,
	type EntryDraft,
	type EntryId,
	Harness,
	type JsonObject,
	MemoryStorage,
	UsageDoc,
} from "@earendil-works/pi-durable";
import { convertToLlm } from "../../core/messages.ts";
import {
	buildSessionContext,
	buildSessionProjection,
	type SessionEntry,
	type SessionHeader,
	sessionEntryToContextMessages,
} from "../../core/session-manager.ts";

export interface LegacyV3Session {
	readonly header: SessionHeader;
	readonly entries: readonly SessionEntry[];
	readonly records: readonly JsonObject[];
	readonly warnings: readonly string[];
}

export interface LegacyV3ImportResult {
	entryIds: Record<string, number>;
	branches: Array<{ conversationId: number; leafId: string; label?: string }>;
	warnings: string[];
}

const context = BACKGROUND_CONTEXT;
const thinkingLevels = new Set(["off", "minimal", "low", "medium", "high", "xhigh", "max"]);

function requireValue(condition: unknown, description: string): asserts condition {
	if (!condition) throw new Error(description);
}

function object(value: unknown, name: string): Record<string, unknown> {
	requireValue(value !== null && typeof value === "object" && !Array.isArray(value), `${name} must be an object`);
	return value as Record<string, unknown>;
}

function text(value: unknown, name: string, nonempty = false): asserts value is string {
	requireValue(
		typeof value === "string" && (!nonempty || value.trim().length > 0),
		`${name} must be ${nonempty ? "a nonempty string" : "a string"}`,
	);
}

function number(value: unknown, name: string): asserts value is number {
	requireValue(
		typeof value === "number" && Number.isFinite(value) && value >= 0,
		`${name} must be a finite nonnegative number`,
	);
}

function timestamp(value: unknown): void {
	text(value, "timestamp", true);
	const parts = /^(\d{4})-(\d{2})-(\d{2})T\d{2}:\d{2}:\d{2}(?:\.\d+)?(?:Z|[+-]\d{2}:\d{2})$/.exec(value);
	requireValue(parts && Number.isFinite(Date.parse(value)), "timestamp must be a valid ISO timestamp");
	const month = Number(parts[2]);
	const day = Number(parts[3]);
	requireValue(
		month >= 1 && month <= 12 && day >= 1 && day <= new Date(Date.UTC(Number(parts[1]), month, 0)).getUTCDate(),
		"timestamp has an invalid calendar date",
	);
}

function usage(value: unknown): void {
	const fields = object(value, "usage");
	for (const key of ["input", "output", "cacheRead", "cacheWrite", "totalTokens"]) number(fields[key], `usage.${key}`);
	for (const key of ["cacheWrite1h", "reasoning"]) if (fields[key] !== undefined) number(fields[key], `usage.${key}`);
	const cost = object(fields.cost, "usage.cost");
	for (const key of ["input", "output", "cacheRead", "cacheWrite", "total"]) number(cost[key], `usage.cost.${key}`);
}

function content(value: unknown, role: string, allowString = role !== "assistant" && role !== "toolResult"): void {
	if (typeof value === "string" && allowString) return;
	requireValue(Array.isArray(value), `${role} content must be ${allowString ? "a string or " : ""}an array`);
	for (const item of value) {
		const block = object(item, "content block");
		switch (block.type) {
			case "text":
				text(block.text, "text content");
				if (block.textSignature !== undefined) text(block.textSignature, "textSignature");
				break;
			case "image":
				requireValue(role !== "assistant" && role !== "system", `${role} cannot contain images`);
				text(block.data, "image.data");
				text(block.mimeType, "image.mimeType", true);
				break;
			case "thinking":
				requireValue(role === "assistant", `${role} cannot contain thinking blocks`);
				text(block.thinking, "thinking content");
				if (block.thinkingSignature !== undefined) text(block.thinkingSignature, "thinkingSignature");
				if (block.redacted !== undefined)
					requireValue(typeof block.redacted === "boolean", "redacted must be boolean");
				break;
			case "toolCall":
				requireValue(role === "assistant", `${role} cannot contain tool calls`);
				text(block.id, "tool call id", true);
				text(block.name, "tool call name", true);
				object(block.arguments, "tool call arguments");
				if (block.thoughtSignature !== undefined) text(block.thoughtSignature, "thoughtSignature");
				if (block.namespace !== undefined) text(block.namespace, "tool namespace");
				break;
			default:
				throw new Error(
					`Unsupported ${role} content block ${String(block.type)}; use the stable CLI for this session`,
				);
		}
	}
}

function message(value: unknown, warnings: string[], id: string): void {
	const item = object(value, "message");
	number(item.timestamp, "message.timestamp");
	requireValue(
		Number.isFinite(new Date(item.timestamp).getTime()),
		"message.timestamp is outside the supported date range",
	);
	switch (item.role) {
		case "system":
			content(item.content, "system");
			if (item.sections !== undefined) {
				for (const section of Object.values(object(item.sections, "system.sections"))) {
					requireValue(section === null || typeof section === "string", "system sections must be strings or null");
				}
			}
			for (const key of ["toolsAdded", "toolsRemoved"]) {
				if (item[key] === undefined) continue;
				requireValue(Array.isArray(item[key]), `system.${key} must be an array`);
				for (const tool of item[key]) {
					const definition = object(tool, "system tool");
					text(definition.name, "system tool name", true);
					if (key === "toolsAdded") {
						text(definition.description, "system tool description");
						object(definition.parameters, "system tool parameters");
					}
				}
				warnings.push(
					`Entry ${id}: historical tool declarations are retained as context only; executable tools come from the current durable registry.`,
				);
			}
			break;
		case "user":
			content(item.content, "user");
			break;
		case "assistant":
			content(item.content, "assistant");
			for (const key of ["api", "provider", "model"]) text(item[key], `assistant.${key}`, true);
			text(item.stopReason, "assistant.stopReason", true);
			requireValue(
				["pending", "stop", "length", "toolUse", "error", "aborted", "deferred"].includes(item.stopReason),
				"assistant.stopReason is invalid",
			);
			usage(item.usage);
			for (const key of ["responseModel", "responseId", "providerThinkingLevel", "errorMessage", "rawStopReason"]) {
				if (item[key] !== undefined) text(item[key], `assistant.${key}`);
			}
			if (item.endTurn !== undefined)
				requireValue(typeof item.endTurn === "boolean", "assistant.endTurn must be boolean");
			if (item.thinkingLevel !== undefined) {
				text(item.thinkingLevel, "assistant.thinkingLevel", true);
				requireValue(thinkingLevels.has(item.thinkingLevel), "assistant.thinkingLevel is unsupported");
			}
			if (item.deferred !== undefined) {
				const handle = object(item.deferred, "assistant.deferred");
				for (const key of ["provider", "modelId", "api", "id"]) text(handle[key], `deferred.${key}`, true);
				for (const key of ["expiresAt", "pollAfterMs"])
					if (handle[key] !== undefined) number(handle[key], `deferred.${key}`);
			}
			if (["aborted", "error", "deferred"].includes(item.stopReason)) {
				warnings.push(
					`Entry ${id}: ${item.stopReason} assistant is retained in history and usage but excluded from durable model context; no work will be resumed.`,
				);
			} else if (item.stopReason === "pending") {
				throw new Error(
					"Pending assistant is not a completed message; finish or repair this session with the stable CLI before importing",
				);
			}
			break;
		case "toolResult":
			content(item.content, "toolResult");
			text(item.toolCallId, "toolCallId", true);
			text(item.toolName, "toolName", true);
			requireValue(typeof item.isError === "boolean", "toolResult.isError must be boolean");
			if (item.usage !== undefined) usage(item.usage);
			break;
		case "custom":
			content(item.content, "custom");
			text(item.customType, "customType", true);
			requireValue(typeof item.display === "boolean", "custom.display must be boolean");
			warnings.push(
				`Entry ${id}: custom message content is retained, but extension state and rendering are archive-only.`,
			);
			break;
		case "bashExecution":
			text(item.command, "bash command");
			text(item.output, "bash output");
			for (const key of ["cancelled", "truncated"])
				requireValue(typeof item[key] === "boolean", `bash ${key} must be boolean`);
			if (item.exitCode !== undefined && item.exitCode !== null)
				requireValue(Number.isInteger(item.exitCode), "bash exitCode must be an integer");
			if (item.fullOutputPath !== undefined) text(item.fullOutputPath, "fullOutputPath");
			if (item.excludeFromContext !== undefined)
				requireValue(typeof item.excludeFromContext === "boolean", "excludeFromContext must be boolean");
			break;
		default:
			throw new Error(
				`Unsupported message role ${String(item.role)}; use the stable CLI or an extension-specific exporter for this session`,
			);
	}
}

export function parseLegacyV3(source: string): LegacyV3Session {
	const lines = source.split("\n");
	if (lines.at(-1) === "") lines.pop();
	const entries: SessionEntry[] = [];
	const records: JsonObject[] = [];
	const warnings: string[] = [];
	const byId = new Map<string, SessionEntry>();
	let header: SessionHeader | undefined;
	for (const [index, line] of lines.entries()) {
		try {
			requireValue(line.trim(), "Blank lines are not valid session records");
			const raw = object(copyJson(JSON.parse(line)), "record");
			text(raw.id, "id", true);
			timestamp(raw.timestamp);
			if (index === 0) {
				requireValue(
					raw.type === "session" && raw.version === 3,
					"Expected a session header with version exactly 3",
				);
				text(raw.cwd, "cwd");
				if (raw.parentSession !== undefined) text(raw.parentSession, "parentSession");
				header = raw as unknown as SessionHeader;
				continue;
			}
			text(raw.type, "type", true);
			requireValue(raw.type !== "session", "Only the first line may be a session header");
			requireValue(!byId.has(raw.id) && raw.id !== header?.id, `Duplicate entry id ${raw.id}`);
			requireValue(
				raw.parentId === null || (typeof raw.parentId === "string" && byId.has(raw.parentId)),
				`Parent ${String(raw.parentId)} must be null or an earlier entry`,
			);
			const ancestors = new Set<string>();
			if (raw.type === "compaction" || raw.type === "context_edit") {
				let parent = raw.parentId === null ? undefined : byId.get(raw.parentId);
				while (parent) {
					ancestors.add(parent.id);
					parent = parent.parentId === null ? undefined : byId.get(parent.parentId);
				}
			}
			let entry = raw as unknown as SessionEntry;
			switch (raw.type) {
				case "message":
					message(raw.message, warnings, raw.id);
					break;
				case "model_change":
					text(raw.provider, "provider", true);
					text(raw.modelId, "modelId", true);
					break;
				case "thinking_level_change":
					text(raw.thinkingLevel, "thinkingLevel", true);
					requireValue(
						thinkingLevels.has(raw.thinkingLevel),
						"Unsupported thinkingLevel; use the stable CLI for this session",
					);
					break;
				case "compaction":
					text(raw.summary, "summary");
					text(raw.firstKeptEntryId, "firstKeptEntryId", true);
					requireValue(
						raw.firstKeptEntryId === raw.id || ancestors.has(raw.firstKeptEntryId),
						"Compaction firstKeptEntryId must be an ancestor or the compaction itself",
					);
					number(raw.tokensBefore, "tokensBefore");
					if (raw.systemMessage !== undefined) {
						requireValue(
							object(raw.systemMessage, "systemMessage").role === "system",
							"Compaction systemMessage must have role system",
						);
						message(raw.systemMessage, warnings, raw.id);
					}
					break;
				case "branch_summary":
					text(raw.summary, "summary");
					text(raw.fromId, "fromId", true);
					requireValue(
						raw.fromId === "root" || byId.has(raw.fromId),
						"Branch summary fromId must refer to an earlier entry or root",
					);
					break;
				case "context_edit": {
					text(raw.targetId, "targetId", true);
					requireValue(ancestors.has(raw.targetId), "Context edit target must be an ancestor");
					const target = byId.get(raw.targetId)!;
					requireValue(
						target.type === "custom_message" ||
							(target.type === "message" && ["user", "assistant", "toolResult"].includes(target.message.role)),
						"Context edit target does not have editable model content",
					);
					if (raw.replacement !== null) {
						content(
							object(raw.replacement, "replacement").content,
							target.type === "message" ? target.message.role : "custom",
							true,
						);
					}
					break;
				}
				case "label":
					text(raw.targetId, "targetId", true);
					requireValue(byId.has(raw.targetId), "Label target must refer to an earlier entry");
					if (raw.label !== undefined) text(raw.label, "label");
					break;
				case "session_info":
					if (raw.name !== undefined) text(raw.name, "name");
					break;
				case "usage":
					for (const key of ["kind", "provider", "model"]) text(raw[key], key, true);
					usage(raw.usage);
					if (raw.note !== undefined) text(raw.note, "note");
					break;
				case "custom_message":
					content(raw.content, "custom");
					requireValue(typeof raw.display === "boolean", "display must be boolean");
					text(raw.customType, "customType", true);
					warnings.push(
						`Entry ${raw.id}: custom message content is retained, but extension state and rendering are archive-only.`,
					);
					break;
				case "custom":
					text(raw.customType, "customType", true);
					warnings.push(
						`Entry ${raw.id}: custom extension state is archive-only and will not be restored or executed.`,
					);
					break;
				default:
					warnings.push(
						`Entry ${raw.id}: unsupported record ${raw.type} is archive-only; its state will not be restored or executed.`,
					);
					entry = {
						type: "custom",
						id: raw.id,
						parentId: raw.parentId,
						timestamp: raw.timestamp as string,
						customType: `legacy-v3:${raw.type}`,
						data: raw,
					};
			}
			if (raw.type === "compaction" || raw.type === "branch_summary") {
				if (raw.usage !== undefined) usage(raw.usage);
				if (raw.fromHook !== undefined) requireValue(typeof raw.fromHook === "boolean", "fromHook must be boolean");
				if (raw.details !== undefined || raw.fromHook)
					warnings.push(
						`Entry ${raw.id}: extension summary metadata is archive-only; the saved summary is retained.`,
					);
			}
			entries.push(entry);
			records.push(raw as JsonObject);
			byId.set(entry.id, entry);
		} catch (error) {
			throw new Error(
				`Invalid v3 session at line ${index + 1}: ${error instanceof Error ? error.message : String(error)}`,
				{ cause: error },
			);
		}
	}
	requireValue(header !== undefined, "Invalid v3 session: missing session header");
	return { header, entries, records, warnings: [...new Set(warnings)] };
}

export async function importLegacyV3(
	session: LegacyV3Session,
	harness: Harness,
	cwd: string,
): Promise<LegacyV3ImportResult> {
	const inspection = await harness.inspect(context);
	requireValue(
		inspection.scheduling === "paused" && inspection.tasks.length === 0 && inspection.submissions.length === 0,
		"Legacy import requires an empty, paused Harness",
	);
	const root = await harness.root(context);
	const conversations = await harness.commit((tx) => tx.scanConversations({}, 2), context);
	requireValue(
		conversations.items.length === 1 && (await root.entries({}, 1, undefined, context)).items.length === 0,
		"Legacy import requires a brand-new empty Harness",
	);
	const entries = [...session.entries];
	const byId = new Map(entries.map((entry) => [entry.id, entry]));
	const records = new Map(session.records.map((record) => [record.id as string, record]));
	const activePath: SessionEntry[] = [];
	let active = entries.at(-1);
	while (active) {
		activePath.push(active);
		active = active.parentId === null ? undefined : byId.get(active.parentId);
	}
	activePath.reverse();
	const warnings = [...session.warnings];
	const seed = await harness.commit(async (tx) => {
		Object.assign(await tx.doc(AgentDoc, root.id), { cwd, thinkingLevel: "off" });
		return tx.appendEntry(root.id, {
			kind: "legacy-v3.session",
			data: copyJson(session.header, { omitUndefinedProperties: true }),
		});
	}, context);
	const mapped = new Map<string, { entryId: EntryId; conversationId: ConversationId }>();
	const tips = new Map<ConversationId, EntryId>([[root.id, seed.id]]);
	for (const entry of [...activePath, ...entries]) {
		if (mapped.has(entry.id)) continue;
		const parent =
			entry.parentId === null ? { entryId: seed.id, conversationId: root.id } : mapped.get(entry.parentId)!;
		const conversationId =
			tips.get(parent.conversationId) === parent.entryId
				? parent.conversationId
				: (
						await harness.commit(
							(tx) =>
								tx.forkConversation(parent.conversationId, parent.entryId, {
									ownership: { kind: "ownerless" },
								}),
							context,
						)
					).id;
		const model = convertToLlm(sessionEntryToContextMessages(entry));
		let kind = `legacy-v3.${entry.type}`;
		let head: EntryDraft["head"];
		const edits: ContextEdit[] = [];
		if (entry.type === "message" || entry.type === "custom_message" || entry.type === "branch_summary") {
			const role = model[0]?.role;
			if (role !== undefined) kind = role === "toolResult" ? "pi.tool-result" : `pi.${role}`;
		} else if (entry.type === "compaction") {
			kind = "pi.compaction";
			head = entry.firstKeptEntryId === entry.id ? "self" : mapped.get(entry.firstKeptEntryId)!.entryId;
			let ancestor = entry.parentId === null ? undefined : byId.get(entry.parentId);
			while (ancestor) {
				if (ancestor.type === "message" && ancestor.message.role === "system")
					edits.push({ target: mapped.get(ancestor.id)!.entryId, action: "omit" });
				ancestor = ancestor.parentId === null ? undefined : byId.get(ancestor.parentId);
			}
		} else if (entry.type === "context_edit") {
			const target = byId.get(entry.targetId)!;
			const targetId = mapped.get(entry.targetId)!.entryId;
			const projected = buildSessionProjection([target, { ...entry, parentId: target.id }], entry.id);
			edits.push(
				entry.replacement === null
					? { target: targetId, action: "omit" }
					: { target: targetId, action: "replace", messages: convertToLlm(projected.entries[0]!.messages) },
			);
		}
		const settings = buildSessionContext(entries, entry.id, byId);
		const added = await harness.commit(async (tx) => {
			const agent = await tx.doc(AgentDoc, conversationId);
			agent.cwd = cwd;
			agent.thinkingLevel = settings.thinkingLevel as ModelThinkingLevel;
			if (settings.model === null) delete agent.model;
			else agent.model = settings.model;
			let spent: Usage | undefined;
			let bucket: "models" | "tools" = "models";
			let key = settings.model === null ? "legacy/unknown" : `${settings.model.provider}/${settings.model.modelId}`;
			if (entry.type === "message" && entry.message.role === "assistant") spent = entry.message.usage;
			else if (entry.type === "message" && entry.message.role === "toolResult") {
				spent = entry.message.usage;
				bucket = "tools";
				key = entry.message.toolName;
			} else if (entry.type === "usage") {
				spent = entry.usage;
				key = `${entry.provider}/${entry.model}`;
			} else if (entry.type === "compaction" || entry.type === "branch_summary") {
				spent = entry.usage;
				if (spent && settings.model === null)
					warnings.push(`Entry ${entry.id}: summary usage has no recorded model; preserved under legacy/unknown.`);
			}
			if (spent) {
				const totals = (await tx.doc(UsageDoc, conversationId))[bucket];
				const total = Object.hasOwn(totals, key) ? totals[key] : undefined;
				if (total === undefined)
					totals[key] = copyJson(spent, { omitUndefinedProperties: true }) as JsonRepresentation<Usage>;
				else {
					for (const field of [
						"input",
						"output",
						"cacheRead",
						"cacheWrite",
						"totalTokens",
						"cacheWrite1h",
						"reasoning",
					] as const) {
						if (spent[field] !== undefined) total[field] = (total[field] ?? 0) + spent[field];
					}
					for (const field of ["input", "output", "cacheRead", "cacheWrite", "total"] as const)
						total.cost[field] += spent.cost[field];
				}
			}
			return tx.appendEntry(conversationId, {
				kind,
				data: copyJson(records.get(entry.id), { omitUndefinedProperties: true }),
				model,
				...(head === undefined ? {} : { head }),
				...(edits.length === 0 ? {} : { edits }),
			});
		}, context);
		mapped.set(entry.id, { entryId: added.id, conversationId });
		tips.set(conversationId, added.id);
	}
	const parents = new Set(entries.map((entry) => entry.parentId));
	const labels = new Map<string, string>();
	for (const entry of entries)
		if (entry.type === "label") {
			if (entry.label) labels.set(entry.targetId, entry.label);
			else labels.delete(entry.targetId);
		}
	const branches: LegacyV3ImportResult["branches"] = [];
	const verifier = await Harness.open(
		new MemoryStorage(),
		{ models: createModels(), registry: createRegistry() },
		context,
	);
	try {
		const expectedConversation = await verifier.root(context);
		for (const entry of entries) {
			if (parents.has(entry.id)) continue;
			const { conversationId } = mapped.get(entry.id)!;
			const expected = convertToLlm(buildSessionContext(entries, entry.id, byId).messages);
			await verifier.commit(
				(tx) =>
					tx.appendEntry(expectedConversation.id, { kind: "legacy-v3.verify", head: "self", model: expected }),
				context,
			);
			const normalized = (await expectedConversation.context(context)).messages;
			const conversation = (await harness.conversation(conversationId, context))!;
			requireValue(
				isDeepStrictEqual((await conversation.context(context)).messages, normalized),
				`Imported context differs from v3 at leaf ${entry.id}; import cannot be published safely`,
			);
			const unmatched = expected.filter((message) => message.role === "toolResult");
			for (const result of normalized) {
				if (result.role !== "toolResult") continue;
				const match = unmatched.findIndex((original) => isDeepStrictEqual(original, result));
				if (match === -1) {
					warnings.push(
						`Leaf ${entry.id}: missing tool result ${result.toolCallId} is represented by a synthetic error in context only; the tool will not be replayed.`,
					);
				} else unmatched.splice(match, 1);
			}
			for (const original of unmatched)
				warnings.push(
					`Leaf ${entry.id}: unmatched or duplicate tool result ${original.toolCallId} is retained in history but omitted from durable model context.`,
				);
			let labeled: SessionEntry | undefined = entry;
			while (labeled && !labels.has(labeled.id))
				labeled = labeled.parentId === null ? undefined : byId.get(labeled.parentId);
			const label = labeled === undefined ? undefined : labels.get(labeled.id);
			branches.push({ conversationId, leafId: entry.id, ...(label === undefined ? {} : { label }) });
		}
	} finally {
		await verifier.close(context);
	}
	requireValue(
		(await harness.commit((tx) => tx.scanTasks({}, 1), context)).items.length === 0,
		"Legacy import unexpectedly created tasks",
	);
	return {
		entryIds: Object.fromEntries([...mapped].map(([id, value]) => [id, value.entryId])),
		branches,
		warnings: [...new Set(warnings)],
	};
}
