import { randomUUID } from "node:crypto";
import type { AuthEvent, AuthPrompt, AuthType } from "@at-inc/pi-ai";
import { clampThinkingLevel, getSupportedThinkingLevels, type ModelThinkingLevel } from "@at-inc/pi-ai";
import type { AttachedReplicatedState } from "@earendil-works/chord";
import { BACKGROUND_CONTEXT } from "@earendil-works/chord/context";
import {
	type AgentState,
	type Conversation,
	type ConversationId,
	type ConversationView,
	type Cursor,
	type EntryRecord,
	Harness,
	type ModelRef,
	ROOT_CONVERSATION_ID,
	type Submission,
	type TaskGraph,
} from "@earendil-works/pi-durable";
import { openNodeSqliteStorage } from "@earendil-works/pi-durable/storage/sqlite/node";
import { DEFAULT_THINKING_LEVEL } from "../../core/defaults.ts";
import { ModelRuntime } from "../../core/model-runtime.ts";
import { SettingsManager } from "../../core/settings-manager.ts";
import {
	configureHarnessHttp,
	createCodingRegistry,
	createHarnessSettings,
	ExecutionEnvs,
	findInitialAgentModel,
} from "./harness-setup.ts";
import { createLoginProviderOptions, type LoginProviderOption } from "./login.ts";
import { selectSession } from "./sessions.ts";
import { Subagent } from "./subagent.ts";

const context = BACKGROUND_CONTEXT;

export interface ModelSummary extends ModelRef {
	readonly name: string;
	readonly contextWindow: number;
}

export interface Notice {
	readonly id: number;
	readonly level: "info" | "warning" | "error";
	readonly message: string;
}

export type AuthPromptRequest = AuthPrompt extends infer Prompt
	? Prompt extends unknown
		? Omit<Prompt, "signal">
		: never
	: never;

export interface DurableAuthView {
	readonly providerId: string;
	readonly providerName: string;
	readonly authType: AuthType;
	readonly notices: readonly AuthEvent[];
	readonly prompt?: { readonly id: string; readonly request: AuthPromptRequest };
}

/** A conversation the user can switch to: the main one, or a subagent's. */
export interface ConversationSummary {
	readonly id: ConversationId;
	readonly label: string;
	/** The first user message, for a subagent its task. */
	readonly title?: string;
}

/** Everything the TUI renders. Plain values; no Harness objects cross this boundary. */
export interface DurableView {
	readonly session: { readonly id: string; readonly directory: string; readonly cwd: string };
	/** The conversation shown and talked to. */
	readonly conversation: ConversationView;
	readonly conversations: readonly ConversationSummary[];
	readonly models: readonly ModelSummary[];
	readonly loginProviders: readonly LoginProviderOption[];
	readonly notices: readonly Notice[];
	readonly auth?: DurableAuthView;
	/** The live task graph while the task panel is open. */
	readonly tasks?: TaskGraph;
}

export interface DurableViewSource {
	current(): DurableView;
	subscribe(listener: () => void): () => void;
}

/** What the TUI may ask for. */
export interface DurableController {
	/** Prompt when idle; otherwise steer or queue a follow-up. */
	submit(text: string, whenBusy: "steer" | "followUp"): Promise<void>;
	compact(instructions: string | undefined): Promise<void>;
	abort(): Promise<void>;
	cycleThinking(): Promise<void>;
	setModel(model: ModelRef): Promise<void>;
	login(providerId: string, authType: AuthType): Promise<void>;
	replyAuth(requestId: string, answer: string | null): Promise<void>;
	cancelLogin(): Promise<void>;
	toggleTasks(): Promise<void>;
	/** Show and talk to another conversation. */
	switchConversation(id: ConversationId): Promise<void>;
}

export interface OpenDurableOptions {
	readonly cwd?: string;
	readonly continueSession?: boolean;
}

export interface OpenDurableResult {
	readonly view: DurableViewSource;
	readonly controller: DurableController;
	/** pi's settings, for the TUI's theme and terminal capabilities. */
	readonly settings: SettingsManager;
	close(): Promise<void>;
}

/** The agent document of a view; absent while the conversation has none. */
export function agentOf(view: ConversationView): AgentState {
	return (view.docs["pi.agent"] ?? {}) as AgentState;
}

/** A subagent's task: the oldest user message of its conversation. The main conversation needs no title. */
async function firstInput(harness: Harness, id: ConversationId): Promise<{ title?: string }> {
	if (id === ROOT_CONVERSATION_ID) return {};
	const conversation = (await harness.conversation(id, context))!;
	let first: EntryRecord | undefined;
	let cursor: Cursor | undefined;
	do {
		const page = await conversation.entries({}, 256, cursor, context);
		first = page.items.findLast((entry) => entry.kind === "pi.user") ?? first;
		cursor = page.next;
	} while (cursor !== undefined);
	return titleOf(first);
}

/** The text of a user entry, as a one-line title. */
function titleOf(entry: EntryRecord | undefined): { title?: string } {
	const message = entry?.model?.[0];
	if (message?.role !== "user") return {};
	const text =
		typeof message.content === "string"
			? message.content
			: message.content.flatMap((block) => (block.type === "text" ? [block.text] : [])).join(" ");
	return { title: text.replace(/\s+/g, " ").trim() };
}

export async function openDurable(options: OpenDurableOptions = {}): Promise<OpenDurableResult> {
	const location = await selectSession(options.cwd ?? process.cwd(), options.continueSession ?? false);
	const envs = new ExecutionEnvs(location.cwd);
	let harness: Harness | undefined;
	try {
		const modelRuntime = await ModelRuntime.create();
		const settingsManager = SettingsManager.create(location.cwd);
		configureHarnessHttp(settingsManager);
		const settings = createHarnessSettings(settingsManager);
		const registry = createCodingRegistry(settingsManager, location.cwd);
		registry.install(Subagent);

		const pendingReports: unknown[] = [];
		let report: (error: unknown) => void = (error) => pendingReports.push(error);
		harness = await Harness.open(
			await openNodeSqliteStorage(location.database),
			{
				models: modelRuntime,
				registry,
				settings,
				env: envs.env,
				onReport: (error) => report(error),
			},
			context,
		);
		const preferredModel =
			location.created &&
			settingsManager.getDefaultProvider() === undefined &&
			settingsManager.getDefaultModel() === undefined
				? modelRuntime
						.getAvailableSnapshot()
						.find((model) => model.provider === "openai-codex" && model.id === "gpt-6-sol")
				: undefined;
		const initial = location.created
			? preferredModel
				? {
						model: { provider: preferredModel.provider, modelId: preferredModel.id },
						thinkingLevel: clampThinkingLevel(
							preferredModel,
							settingsManager.getDefaultThinkingLevel() ?? DEFAULT_THINKING_LEVEL,
						),
						fallbackMessage: undefined,
					}
				: await findInitialAgentModel(settingsManager, modelRuntime)
			: undefined;
		const root = await harness.root(context, {
			agent: {
				cwd: location.cwd,
				...(initial?.model === undefined ? {} : { model: initial.model }),
				...(initial?.thinkingLevel === undefined ? {} : { thinkingLevel: initial.thinkingLevel }),
			},
		});
		const label = (id: ConversationId): string => (id === root.id ? "main" : `subagent ${id}`);
		const opened = harness;
		const summaries: ConversationSummary[] = [];
		let cursor: Cursor | undefined;
		do {
			const page = await opened.commit((tx) => tx.scanConversations({}, 256, cursor), context);
			for (const { id } of page.items) summaries.push({ id, label: label(id), ...(await firstInput(opened, id)) });
			cursor = page.next;
		} while (cursor !== undefined);
		let current: Conversation = root;
		let conversation: AttachedReplicatedState<ConversationView> = await root.viewState(context);
		const models = (): ModelSummary[] =>
			modelRuntime.getAvailableSnapshot().map((model) => ({
				provider: model.provider,
				modelId: model.id,
				name: model.name,
				contextWindow: model.contextWindow,
			}));
		const loginProviders = (): LoginProviderOption[] =>
			createLoginProviderOptions(
				modelRuntime.getProviders().map((provider) => ({
					id: provider.id,
					name: provider.name,
					...(provider.auth.oauth === undefined
						? {}
						: {
								oauth: {
									name: provider.auth.oauth.name,
									...(provider.auth.oauth.isSubscription === undefined
										? {}
										: { subscription: provider.auth.oauth.isSubscription }),
								},
							}),
					...(provider.auth.apiKey === undefined
						? {}
						: {
								apiKey: {
									name: provider.auth.apiKey.name,
									interactive: provider.auth.apiKey.login !== undefined,
								},
							}),
				})),
				(providerId) => modelRuntime.getProviderAuthStatus(providerId),
				(providerId) => modelRuntime.isUsingOAuth(providerId),
			);

		let state: DurableView = {
			session: { id: location.id, directory: location.directory, cwd: location.cwd },
			conversation: conversation.value,
			conversations: summaries,
			models: models(),
			loginProviders: loginProviders(),
			notices: [],
		};
		const listeners = new Set<() => void>();
		let notifying = false;
		// Commit listeners and Chord frames call this on the Session line; rendering runs afterwards, once per burst.
		const update = (patch: Partial<DurableView>): void => {
			state = { ...state, ...patch };
			if (notifying) return;
			notifying = true;
			setImmediate(() => {
				notifying = false;
				for (const listener of listeners) listener();
			});
		};
		let nextNotice = 1;
		const notice = (level: Notice["level"], message: string): void => {
			update({ notices: [...state.notices, { id: nextNotice++, level, message }].slice(-20) });
		};
		const fail = (error: unknown): void => notice("error", error instanceof Error ? error.message : String(error));
		report = (error) => notice("warning", error instanceof Error ? error.message : String(error));
		for (const error of pendingReports) report(error);
		let unsubscribe = conversation.subscribe((value) => update({ conversation: value }));
		// Subagents appear as their conversations are created. A commit listener only records; it calls no Session API.
		const unsubscribeCommits = harness.subscribeCommits((publication) => {
			let conversations = state.conversations;
			for (const change of publication.changes) {
				if (change.type === "conversation") {
					conversations = [...conversations, { id: change.value.id, label: label(change.value.id) }];
				} else if (change.type === "entry" && change.value.kind === "pi.user") {
					const id = change.value.conversationId;
					conversations = conversations.map((summary) =>
						summary.id === id && summary.title === undefined ? { ...summary, ...titleOf(change.value) } : summary,
					);
				}
			}
			if (conversations !== state.conversations) update({ conversations });
		});

		let tasks: AttachedReplicatedState<TaskGraph> | undefined;
		let unsubscribeTasks = (): void => {};
		const closeTasks = (): void => {
			unsubscribeTasks();
			tasks?.dispose();
			tasks = undefined;
		};

		let queue = Promise.resolve();
		// One at a time, so toggles, switches, and key presses apply in order.
		const command = (operation: () => Promise<void>): Promise<void> => {
			queue = queue.then(operation).catch(fail);
			return queue;
		};
		const watchAnswer = (submission: Submission): void => {
			void submission.wait(context).then((settled) => {
				if (settled.status === "unanswered" && settled.reason !== "aborted") {
					notice(
						"error",
						`No answer: ${settled.reason}${settled.detail === undefined ? "" : ` ${JSON.stringify(settled.detail)}`}`,
					);
				}
			}, fail);
		};
		const agentModel = () => {
			const ref = agentOf(state.conversation).model;
			const model = ref === undefined ? undefined : modelRuntime.getModel(ref.provider, ref.modelId);
			if (model === undefined)
				throw new Error(ref === undefined ? "No model selected" : "Current model is unavailable");
			return model;
		};
		let loginController: AbortController | undefined;
		let loginOperation: Promise<void> | undefined;
		let pendingAuth:
			| {
					id: string;
					resolve(answer: string): void;
					reject(error: Error): void;
					signal?: AbortSignal;
					onAbort(): void;
			  }
			| undefined;
		const clearPendingAuth = (error: Error): void => {
			const pending = pendingAuth;
			pendingAuth = undefined;
			if (pending === undefined) return;
			pending.signal?.removeEventListener("abort", pending.onAbort);
			pending.reject(error);
		};
		const askAuth = (request: AuthPrompt, signal?: AbortSignal): Promise<string> => {
			if (signal?.aborted) return Promise.reject(new Error("Login cancelled"));
			clearPendingAuth(new Error("Login prompt replaced"));
			const { signal: _signal, ...prompt } = request;
			return new Promise((resolve, reject) => {
				const id = randomUUID();
				const onAbort = (): void => {
					if (pendingAuth?.id !== id) return;
					pendingAuth = undefined;
					update({ auth: state.auth ? { ...state.auth, prompt: undefined } : undefined });
					reject(new Error("Login cancelled"));
				};
				pendingAuth = { id, resolve, reject, signal, onAbort };
				signal?.addEventListener("abort", onAbort, { once: true });
				if (state.auth) update({ auth: { ...state.auth, prompt: { id, request: prompt } } });
			});
		};
		const addAuthNotice = (authEvent: AuthEvent): void => {
			if (state.auth)
				update({ auth: { ...state.auth, notices: [...state.auth.notices, structuredClone(authEvent)] } });
		};
		const controller: DurableController = {
			submit: (text, whenBusy) =>
				command(async () => watchAnswer(await current.submit({ type: "input", content: text, whenBusy }, context))),
			compact: (instructions) =>
				command(async () => {
					const id = await current.compact(instructions, context);
					// Report the outcome once it is known; the status line shows the compaction meanwhile.
					void opened.waitForTask(id, context).then(async (receipt) => {
						const outcome = receipt.state.outcome;
						if (outcome.status === "completed") {
							const { entryId, submissionId } = outcome.result;
							// A summary written while busy is a submission: placed now, queued, or dropped as stale.
							const status =
								submissionId === undefined
									? undefined
									: (await (await opened.submission(submissionId, context))?.status(context))?.status;
							notice(
								"info",
								entryId !== undefined || status === "done"
									? "Compacted."
									: status === "queued"
										? "Compaction summary queued; it is placed at the next turn boundary."
										: status === "unanswered"
											? "Compaction summary dropped: the context changed under it."
											: "Nothing to compact: the context fits in the recent window that is kept verbatim.",
							);
						} else if (outcome.status === "aborted") notice("info", "Compaction aborted.");
						else
							notice("error", `Compaction ${outcome.status}: ${outcome.error?.message ?? outcome.reason ?? ""}`);
					}, fail);
				}),
			// Not queued: it waits until the conversation is idle.
			abort: () => current.abort(context).catch(fail),
			cycleThinking: () =>
				command(async () => {
					const model = agentModel();
					if (!model.reasoning) throw new Error("Current model does not support thinking");
					const levels = getSupportedThinkingLevels(model);
					const level = agentOf(state.conversation).thinkingLevel ?? "off";
					const next = levels[(levels.indexOf(level) + 1) % levels.length] ?? "off";
					await current.configure({ thinkingLevel: next }, context);
				}),
			setModel: (ref) =>
				command(async () => {
					const model = modelRuntime.getModel(ref.provider, ref.modelId);
					if (model === undefined) throw new Error(`Unknown model: ${ref.provider}/${ref.modelId}`);
					const thinking: ModelThinkingLevel = agentOf(state.conversation).thinkingLevel ?? "off";
					await current.configure({ model: ref, thinkingLevel: clampThinkingLevel(model, thinking) }, context);
				}),
			login: async (providerId, authType) => {
				if (loginOperation !== undefined) {
					notice("error", "A login is already running");
					return;
				}
				const account = state.loginProviders.find(
					(provider) => provider.id === providerId && provider.authType === authType,
				);
				if (account === undefined) {
					notice("error", `Unknown login method: ${providerId}/${authType}`);
					return;
				}
				if (!account.interactive) {
					notice("info", `${account.methodName} is configured outside pi.`);
					return;
				}
				const activeLogin = new AbortController();
				loginController = activeLogin;
				update({ auth: { providerId, providerName: account.name, authType, notices: [] } });
				const operation = (async () => {
					try {
						await modelRuntime.login(
							providerId,
							authType,
							{
								signal: activeLogin.signal,
								prompt: (request) => askAuth(request, request.signal),
								notify: addAuthNotice,
							},
							{ getDeviceId: () => settingsManager.getOrCreateDeviceId() },
						);
						notice("info", `Logged in to ${account.name}.`);
					} catch (error) {
						if (!activeLogin.signal.aborted && !(error instanceof Error && error.message === "Login cancelled")) {
							fail(error);
						}
					} finally {
						clearPendingAuth(new Error("Login finished"));
						loginController = undefined;
						update({ auth: undefined, models: models(), loginProviders: loginProviders() });
					}
				})();
				loginOperation = operation;
				await operation;
				if (loginOperation === operation) loginOperation = undefined;
			},
			replyAuth: async (requestId, answer) => {
				const pending = pendingAuth;
				if (!pending || pending.id !== requestId) return;
				pendingAuth = undefined;
				pending.signal?.removeEventListener("abort", pending.onAbort);
				update({ auth: state.auth ? { ...state.auth, prompt: undefined } : undefined });
				if (answer === null) pending.reject(new Error("Login cancelled"));
				else pending.resolve(answer);
			},
			cancelLogin: async () => {
				loginController?.abort(new Error("Login cancelled"));
				clearPendingAuth(new Error("Login cancelled"));
				update({ auth: undefined });
			},
			toggleTasks: () =>
				command(async () => {
					if (tasks !== undefined) {
						closeTasks();
						update({ tasks: undefined });
						return;
					}
					const graph = await opened.taskGraph(context);
					tasks = graph;
					unsubscribeTasks = graph.subscribe((value) => update({ tasks: value }));
				}),
			switchConversation: (id) =>
				command(async () => {
					const next = await opened.conversation(id, context);
					if (next === undefined) throw new Error(`Conversation ${id} does not exist`);
					const nextState = await next.viewState(context);
					unsubscribe();
					conversation.dispose();
					current = next;
					conversation = nextState;
					unsubscribe = nextState.subscribe((value) => update({ conversation: value }));
				}),
		};

		const saved = agentOf(state.conversation).model;
		if (saved === undefined) notice("warning", "No model configured; select one with /model.");
		else if (modelRuntime.getModel(saved.provider, saved.modelId) === undefined) {
			notice("warning", `Saved model is unavailable: ${saved.provider}/${saved.modelId}`);
		}
		if (initial?.fallbackMessage !== undefined) notice("info", initial.fallbackMessage);
		// The task panel starts open; /tasks hides it.
		await controller.toggleTasks();
		// Recovered work from an interrupted turn continues now.
		harness.resume();

		let closing: Promise<void> | undefined;
		return {
			view: {
				current: () => state,
				subscribe: (listener) => {
					listeners.add(listener);
					return () => listeners.delete(listener);
				},
			},
			controller,
			settings: settingsManager,
			close() {
				closing ??= (async () => {
					loginController?.abort(new Error("Durable agent closed"));
					clearPendingAuth(new Error("Durable agent closed"));
					await loginOperation?.catch(() => {});
					unsubscribe();
					unsubscribeCommits();
					conversation.dispose();
					closeTasks();
					try {
						// Close writes no outcome: a running turn resumes with --continue.
						await opened.close(context);
						await envs.cleanup(context);
					} finally {
						await location.release();
					}
				})();
				return closing;
			},
		};
	} catch (error) {
		await harness?.close(context).catch(() => {});
		await location.release().catch(() => {});
		throw error;
	}
}
