import type { Message, Model, Models } from "@at-inc/pi-ai";
import { clampMaxTokensToContext } from "@at-inc/pi-ai/api/simple-options";
import { estimateContextTokens } from "@at-inc/pi-ai/utils/estimate";
import { getCurrentSystemMessage, normalizeContext } from "@at-inc/pi-ai/utils/transcript";
import { copyJson, type Draft } from "@earendil-works/chord";
import { defineDoc } from "../documents.ts";
import type { ConversationId, ConversationRecord, EntryDraft, EntryId, EntryRecord, Tx } from "../types.ts";
import { type CommitContext, extendContext } from "./context.ts";
import type {
	Agent,
	AgentChange,
	AgentState,
	CompactionPolicy,
	ConversationRetryPolicy,
	Extension,
	HarnessSettings,
	PromptSection,
	RegistrySnapshot,
	Settings,
	ToolRegistration,
} from "./types.ts";

export const DEFAULT_RETRY_POLICY: ConversationRetryPolicy = {
	enabled: true,
	maxRetries: 3,
	baseDelayMs: 2000,
	maxAgentDelayMs: 60000,
};

export const DEFAULT_COMPACTION_POLICY: CompactionPolicy = {
	enabled: true,
	reserveTokens: 16384,
	keepRecentTokens: 20000,
	backgroundTokens: 32768,
};

/** The reserved section key of the agent's `instructions`. */
export const INSTRUCTIONS_KEY = "instructions";

/** Built-in agent document; rewindable so forks start from the agent at their fork entry. */
export const AgentDoc = defineDoc<AgentState>({
	kind: "pi.agent",
	version: 1,
	scope: "conversation",
	history: "rewindable",
	fork: "asOf",
	initial: () => ({}),
	checkpointWhen: () => true,
});

/** Resolve the host settings: every field over its built-in default, object fields merged. */
export function resolveSettings(settings: HarnessSettings | undefined): Settings {
	const extensions = settings?.extensions;
	return {
		...(extensions === undefined ? {} : { extensions }),
		stream: { ...settings?.stream },
		retry: { ...DEFAULT_RETRY_POLICY, ...settings?.retry },
		compaction: { ...DEFAULT_COMPACTION_POLICY, ...settings?.compaction },
		...(settings?.minimumAnswerTokens === undefined
			? {}
			: { minimumAnswerTokens: Math.max(0, settings.minimumAnswerTokens) }),
		toolExecution: settings?.toolExecution ?? "parallel",
		steeringMode: settings?.steeringMode ?? "one-at-a-time",
		followUpMode: settings?.followUpMode ?? "one-at-a-time",
	};
}

export function requestFits(
	model: Model<string>,
	messages: readonly Message[],
	settings: Settings,
	maxTokens?: number,
): boolean {
	const floor = settings.minimumAnswerTokens ?? 0;
	if (floor <= 0) return true;
	const requested =
		maxTokens ??
		("maxTokens" in settings.stream && typeof settings.stream.maxTokens === "number"
			? settings.stream.maxTokens
			: model.maxTokens);
	const ceilings = [floor, requested, model.maxTokens].filter((tokens) => tokens > 0);
	const minimum = Math.min(...ceilings);
	const transcript = normalizeContext({ messages: [...messages] });
	return (
		clampMaxTokensToContext(model, transcript, requested > 0 ? requested : Number.POSITIVE_INFINITY) >= minimum &&
		(model.contextWindow <= 0 || estimateContextTokens(transcript).tokens + minimum < model.contextWindow)
	);
}

export function compactionFits(
	view: CommitContext,
	entry: EntryDraft,
	agent: Readonly<AgentState>,
	models: Models,
	settings: Settings,
	following: readonly EntryDraft[] = [],
): boolean {
	if ((settings.minimumAnswerTokens ?? 0) <= 0) return true;
	const ref = agent.model;
	const model = ref === undefined ? undefined : models.getModel(ref.provider, ref.modelId);
	if (model === undefined || typeof entry.head !== "number") return false;
	const latest = view.range.at(-1);
	if (latest === undefined) return false;
	const marker = {
		...summaryAtBoundary(view, entry, entry.model?.[0]?.timestamp ?? 0, following),
		head: entry.head,
		id: (latest.id + 1) as EntryId,
		conversationId: latest.conversationId,
	} satisfies EntryRecord;
	let projected = extendContext(view, marker);
	for (const draft of following) {
		if (typeof draft.head === "number" && projected.head?.head !== undefined && draft.head < projected.head.head)
			continue;
		const id = (projected.range.at(-1)!.id + 1) as EntryId;
		projected = extendContext(projected, {
			...draft,
			id,
			conversationId: latest.conversationId,
			head: draft.head === "self" ? id : draft.head,
		});
	}
	const messages: Message[] = projected.messages.filter((message) => message.role !== "system");
	const system = getCurrentSystemMessage([...view.messages, ...following.flatMap((draft) => draft.model ?? [])]);
	const sections = { ...system?.sections };
	if (agent.instructions === undefined) delete sections[INSTRUCTIONS_KEY];
	else sections[INSTRUCTIONS_KEY] = `<${INSTRUCTIONS_KEY}>\n${agent.instructions}\n</${INSTRUCTIONS_KEY}>`;
	if (system !== undefined || Object.keys(sections).length > 0)
		messages.push({
			...system,
			role: "system",
			content: system?.content ?? "",
			sections,
			timestamp: entry.model?.[0]?.timestamp ?? 0,
		});
	return requestFits(model, messages, settings);
}

export function summaryAtBoundary(
	view: CommitContext,
	entry: EntryDraft,
	now: number,
	following: readonly EntryDraft[] = [],
): EntryDraft {
	const timestamp = [...view.messages, ...following.flatMap((draft) => draft.model ?? [])].reduce(
		(latest, message) => Math.max(latest, message.timestamp + 1),
		now,
	);
	return { ...entry, model: entry.model?.map((message) => ({ ...message, timestamp })) };
}

/** Apply one change to `pi.agent`: a given field replaces the stored one, `null` clears it, `undefined` changes nothing. */
export async function configure(tx: Tx, conversationId: ConversationId, change: AgentChange): Promise<void> {
	const state = await tx.doc(AgentDoc, conversationId);
	applyChange(state, change);
}

/**
 * `addTools` of a tool round: an array gets each name it lacks appended, `{ remove }` loses the names, and unset tools
 * already offer every tool, so nothing is written.
 */
export async function addTools(tx: Tx, conversationId: ConversationId, added: readonly string[]): Promise<void> {
	const state = await tx.doc(AgentDoc, conversationId);
	const tools = state.tools;
	if (tools === undefined) return;
	if (Array.isArray(tools)) {
		for (const name of added) if (!tools.includes(name)) tools.push(name);
	} else {
		const remove = (tools as { remove: string[] }).remove;
		if (remove.some((name) => added.includes(name))) {
			state.tools = { remove: remove.filter((name) => !added.includes(name)) };
		}
	}
}

function applyChange(state: Draft<AgentState>, change: AgentChange): void {
	const set = <K extends keyof AgentState>(key: K, value: AgentState[K] | null | undefined) => {
		if (value === undefined) return;
		if (value === null) delete state[key];
		else state[key] = value as Draft<AgentState>[K];
	};
	set("model", change.model === undefined || change.model === null ? change.model : { ...change.model });
	set("thinkingLevel", change.thinkingLevel);
	const extensions = change.extensions;
	set(
		"extensions",
		extensions === undefined || extensions === null
			? extensions
			: isList(extensions)
				? names(extensions)
				: {
						...(extensions.add === undefined ? {} : { add: names(extensions.add) }),
						...(extensions.remove === undefined ? {} : { remove: names(extensions.remove) }),
					},
	);
	const tools = change.tools;
	set(
		"tools",
		tools === undefined || tools === null ? tools : isList(tools) ? names(tools) : { remove: names(tools.remove) },
	);
	set("instructions", change.instructions);
	set("cwd", change.cwd);
}

function isList<T>(value: readonly T[] | object): value is readonly T[] {
	return Array.isArray(value);
}

function names(items: readonly { readonly name: string }[]): string[] {
	return items.map((item) => item.name);
}

/**
 * Built-in part of every Harness commit that creates or forks a conversation, for `pi.agent`: a fork keeps its `asOf`
 * copy; a new task-owned conversation copies the stored agent of its owner task's conversation; a new ownerless one
 * starts empty.
 */
export async function createAgent(tx: Tx, conversation: ConversationRecord): Promise<void> {
	if (conversation.parent !== undefined) return;
	const agent = await tx.doc(AgentDoc, conversation.id);
	if (conversation.owner === undefined) return;
	const owner = await tx.doc(AgentDoc, conversation.owner.conversationId);
	Object.assign(agent, copyJson(owner) as AgentState);
}

/** Handlers of the selected extensions' hooks for a task name, in extension order. */
export function agentHooks(agent: Agent, taskName: string): object[] {
	const handlers: object[] = [];
	for (const extension of agent.extensions) {
		for (const hook of extension.hooks ?? []) if (hook.task === taskName) handlers.push(hook.handlers);
	}
	return handlers;
}

/**
 * Resolve an agent from its stored state (absent: every field unset), a registry snapshot, and resolved settings. A
 * wrapper that throws or renames drops its target and is reported; a wrapper without a target does nothing.
 */
export function resolveAgent<Tool extends ToolRegistration>(
	state: Readonly<AgentState> | undefined,
	snapshot: RegistrySnapshot<Tool>,
	settings: Settings,
	report: (error: unknown) => void,
): Agent<Tool> {
	const extensions = selectExtensions(state?.extensions, snapshot, settings);

	const composed = new Map<string, Tool>();
	for (const extension of extensions) for (const tool of extension.tools ?? []) composed.set(tool.name, tool);
	const sections = new Map<string, PromptSection<Tool>>();
	for (const extension of extensions) {
		for (const section of extension.sections ?? []) sections.set(section.key, section);
	}
	for (const extension of extensions) {
		for (const wrap of extension.wraps ?? []) {
			if ("tool" in wrap)
				applyWrap(
					composed,
					wrap.tool,
					(tool) => wrap.wrap(tool),
					(tool) => tool.name,
					report,
				);
			else
				applyWrap(
					sections,
					wrap.section,
					(section) => wrap.wrap(section),
					(section) => section.key,
					report,
				);
		}
	}

	const filter = state?.tools;
	let tools: Tool[];
	if (filter === undefined) tools = [...composed.values()];
	else if (Array.isArray(filter)) {
		tools = [];
		for (const name of new Set(filter)) {
			const tool = composed.get(name);
			if (tool !== undefined) tools.push(tool);
		}
	} else {
		const removed = new Set((filter as { remove: string[] }).remove);
		tools = [...composed.values()].filter((tool) => !removed.has(tool.name));
	}

	const instructions = state?.instructions;
	const agentSections = [...sections.values()];
	if (instructions !== undefined) agentSections.push({ key: INSTRUCTIONS_KEY, render: () => instructions });

	const agent: Agent<Tool> = {
		...(state?.model === undefined ? {} : { model: state.model }),
		thinkingLevel: state?.thinkingLevel ?? "off",
		extensions,
		tools,
		sections: agentSections,
		...(instructions === undefined ? {} : { instructions }),
		...(state?.cwd === undefined ? {} : { cwd: state.cwd }),
	};
	return agent;
}

/** Selected installed extensions: the stored array, or the default selection edited by `{ add, remove }`. */
function selectExtensions<Tool extends ToolRegistration>(
	stored: AgentState["extensions"],
	snapshot: RegistrySnapshot<Tool>,
	settings: Settings,
): Extension<Tool>[] {
	let selected: string[];
	if (Array.isArray(stored)) selected = stored;
	else {
		const base = settings.extensions?.map((extension) => extension.name) ?? snapshot.installed().map((e) => e.name);
		const edit = stored as { add?: string[]; remove?: string[] } | undefined;
		const removed = new Set(edit?.remove ?? []);
		selected = [...base, ...(edit?.add ?? [])].filter((name) => !removed.has(name));
	}
	const extensions: Extension<Tool>[] = [];
	for (const name of new Set(selected)) {
		const extension = snapshot.extension(name);
		if (extension !== undefined) extensions.push(extension);
	}
	return extensions;
}

function applyWrap<T>(
	items: Map<string, T>,
	target: string,
	wrap: (item: T) => T,
	nameOf: (item: T) => string,
	report: (error: unknown) => void,
): void {
	const item = items.get(target);
	if (item === undefined) return;
	try {
		const wrapped = wrap(item);
		if (nameOf(wrapped) !== target) throw new Error(`Wrapper renamed ${target} to ${nameOf(wrapped)}`);
		items.set(target, wrapped);
	} catch (error) {
		items.delete(target);
		report(error);
	}
}
