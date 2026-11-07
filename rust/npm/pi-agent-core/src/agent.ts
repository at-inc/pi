// The facade of agent.ts: `Agent` over the Rust agent (`AgentHandle`, see rust/crates/pi-wasm/src/agent/agent.rs).
//
// The facade keeps `state` with the caller's objects and applies each event of a run to it, as the TS does, so the
// state is read without crossing. Before a run it sends what changed since the last one: the model, the thinking
// level, and the messages from the first one that differs. The tools, the stream function and the public properties
// go with the run, as the TS reads them when the run starts.

import type * as ts from "@at-inc/pi-agent-core";
import type {
	AgentEvent,
	AgentInitialState,
	AgentMessage,
	AgentOptions,
	AgentState,
	AgentTool,
	QueueMode,
} from "@at-inc/pi-agent-core";
import type { ImageContent, Message, Model } from "@at-inc/pi-ai";
import { wasm } from "@at-inc/pi-core";
import type { AgentHandle } from "@at-inc/pi-core/wasm";
import { AgentEvents } from "./internal/agent-events.ts";
import { getDefaultStreamFn } from "./stream-fn.ts";

export type { AgentInitialState, AgentOptions, QueueMode } from "@at-inc/pi-agent-core";

function defaultConvertToLlm(messages: AgentMessage[]): Message[] {
	return messages.filter(
		(message) =>
			message.role === "system" ||
			message.role === "user" ||
			message.role === "assistant" ||
			message.role === "toolResult",
	);
}

const DEFAULT_MODEL = {
	id: "unknown",
	name: "unknown",
	api: "unknown",
	provider: "unknown",
	baseUrl: "",
	reasoning: false,
	input: [],
	cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0 },
	contextWindow: 0,
	maxTokens: 0,
} satisfies Model<any>;

type MutableAgentState = Omit<AgentState, "isStreaming" | "streamingMessage" | "pendingToolCalls" | "errorMessage"> & {
	isStreaming: boolean;
	streamingMessage?: AgentMessage;
	pendingToolCalls: Set<string>;
	errorMessage?: string;
};

/** The TS state object over `messages`, which already holds the leading system message the agent added. */
function createMutableAgentState(
	initialState: AgentInitialState | undefined,
	messages: AgentMessage[],
): MutableAgentState {
	let tools = initialState?.tools?.slice() ?? [];
	return {
		get systemPrompt() {
			return wasm.agentCurrentSystemPrompt(messages);
		},
		model: initialState?.model ?? DEFAULT_MODEL,
		thinkingLevel: initialState?.thinkingLevel ?? "off",
		get tools() {
			return tools;
		},
		set tools(nextTools: AgentTool<any>[]) {
			tools = nextTools.slice();
		},
		get messages() {
			return messages;
		},
		set messages(nextMessages: AgentMessage[]) {
			messages = nextMessages.slice();
		},
		isStreaming: false,
		streamingMessage: undefined,
		pendingToolCalls: new Set<string>(),
		errorMessage: undefined,
	};
}

type Listener = (event: AgentEvent, signal: AbortSignal) => Promise<void> | void;

/** The state as the Rust agent has it: what the facade sent and the messages of the runs' `message_end` events. */
interface Synced {
	model: Model<any>;
	thinkingLevel: AgentState["thinkingLevel"];
	messages: AgentMessage[];
}

/**
 * Stateful wrapper around the low-level agent loop.
 *
 * `Agent` owns the current transcript, emits lifecycle events, executes tools,
 * and exposes queueing APIs for steering and follow-up messages.
 */
class FacadeAgent {
	readonly #handle: AgentHandle;
	readonly #state: MutableAgentState;
	readonly #synced: Synced;
	readonly #listeners = new Set<Listener>();
	readonly #events = new AgentEvents((event) => this.#processEvents(event));
	#activeRun: { promise: Promise<void>; resolve: () => void } | undefined;

	public convertToLlm: ts.Agent["convertToLlm"];
	public transformContext?: ts.Agent["transformContext"];
	public streamFunction: ts.Agent["streamFunction"];
	public getApiKey?: ts.Agent["getApiKey"];
	public onPayload?: ts.Agent["onPayload"];
	public onResponse?: ts.Agent["onResponse"];
	public onProviderStreamEvent?: ts.Agent["onProviderStreamEvent"];
	public beforeToolCall?: ts.Agent["beforeToolCall"];
	public afterToolCall?: ts.Agent["afterToolCall"];
	public finishTurn?: ts.Agent["finishTurn"];
	public prepareRequest?: ts.Agent["prepareRequest"];
	public prepareNextTurn?: ts.Agent["prepareNextTurn"];
	public prepareNextTurnWithContext?: ts.Agent["prepareNextTurnWithContext"];
	/** Session identifier forwarded to providers for cache-aware backends. */
	public sessionId?: string;
	/** Optional per-level thinking token budgets forwarded to the stream function. */
	public thinkingBudgets?: ts.Agent["thinkingBudgets"];
	/** Preferred transport forwarded to the stream function. */
	public transport: ts.Agent["transport"];
	/** Optional cap for provider-requested retry delays. */
	public maxRetryDelayMs?: number;
	/** Tool execution strategy for assistant messages that contain multiple tool calls. */
	public toolExecution: ts.Agent["toolExecution"];
	/** Request identity forwarded to the stream function, over `DEFAULT_AGENT_IDENTITY`. */
	public identity?: ts.Agent["identity"];

	constructor(options: AgentOptions) {
		// Older compiled consumers may omit options or streamFn even though the current API requires them.
		const runtimeOptions: Partial<AgentOptions> = options ?? {};
		const initialState = runtimeOptions.initialState;
		this.streamFunction = runtimeOptions.streamFn ?? getDefaultStreamFn();
		this.#handle = new wasm.AgentHandle(initialState, runtimeOptions.steeringMode, runtimeOptions.followUpMode);
		const messages = initialState?.messages?.slice() ?? [];
		const inserted = this.#handle.insertedMessage(messages.length);
		if (inserted !== undefined) messages.unshift(inserted);
		this.#state = createMutableAgentState(initialState, messages);
		this.#synced = { model: this.#state.model, thinkingLevel: this.#state.thinkingLevel, messages: messages.slice() };
		this.convertToLlm = runtimeOptions.convertToLlm ?? defaultConvertToLlm;
		this.transformContext = runtimeOptions.transformContext;
		this.getApiKey = runtimeOptions.getApiKey;
		this.onPayload = runtimeOptions.onPayload;
		this.onResponse = runtimeOptions.onResponse;
		this.onProviderStreamEvent = runtimeOptions.onProviderStreamEvent;
		this.beforeToolCall = runtimeOptions.beforeToolCall;
		this.afterToolCall = runtimeOptions.afterToolCall;
		this.finishTurn = runtimeOptions.finishTurn;
		this.prepareRequest = runtimeOptions.prepareRequest;
		this.prepareNextTurn = runtimeOptions.prepareNextTurn;
		this.prepareNextTurnWithContext = runtimeOptions.prepareNextTurnWithContext;
		this.sessionId = runtimeOptions.sessionId;
		this.thinkingBudgets = runtimeOptions.thinkingBudgets;
		this.transport = runtimeOptions.transport ?? "auto";
		this.maxRetryDelayMs = runtimeOptions.maxRetryDelayMs;
		this.toolExecution = runtimeOptions.toolExecution ?? "parallel";
		this.identity = runtimeOptions.identity;
	}

	/**
	 * Subscribe to agent lifecycle events.
	 *
	 * Listener promises are awaited in subscription order and are included in
	 * the current run's settlement. Listeners also receive the active abort
	 * signal for the current run.
	 *
	 * `agent_end` is the final emitted event for a run, but the agent does not
	 * become idle until all awaited listeners for that event have settled.
	 */
	subscribe(listener: Listener): () => void {
		this.#listeners.add(listener);
		return () => this.#listeners.delete(listener);
	}

	/**
	 * Current agent state.
	 *
	 * Assigning `state.tools` or `state.messages` copies the provided top-level array.
	 */
	get state(): AgentState {
		return this.#state;
	}

	/** Controls how queued steering messages are drained. */
	set steeringMode(mode: QueueMode) {
		this.#handle.steeringMode = mode;
	}

	get steeringMode(): QueueMode {
		return this.#handle.steeringMode;
	}

	/** Controls how queued follow-up messages are drained. */
	set followUpMode(mode: QueueMode) {
		this.#handle.followUpMode = mode;
	}

	get followUpMode(): QueueMode {
		return this.#handle.followUpMode;
	}

	/** Queue a message to be injected after the current assistant turn finishes. */
	steer(message: AgentMessage): void {
		this.#handle.steer(message);
	}

	/** Queue a message to run only after the agent would otherwise stop. */
	followUp(message: AgentMessage): void {
		this.#handle.followUp(message);
	}

	/** Remove all queued steering messages. */
	clearSteeringQueue(): void {
		this.#handle.clearSteeringQueue();
	}

	/** Remove all queued follow-up messages. */
	clearFollowUpQueue(): void {
		this.#handle.clearFollowUpQueue();
	}

	/** Remove all queued steering and follow-up messages. */
	clearAllQueues(): void {
		this.clearSteeringQueue();
		this.clearFollowUpQueue();
	}

	/** Returns true when either queue still contains pending messages. */
	hasQueuedMessages(): boolean {
		return this.#handle.hasQueuedMessages();
	}

	/** Preview the messages selected for the next turn without consuming them. */
	peekQueuedMessages(): AgentMessage[] {
		return this.#handle.peekQueuedMessages();
	}

	/** Active abort signal for the current run, if any. */
	get signal(): AbortSignal | undefined {
		return this.#handle.signal();
	}

	/** Abort the current run, if one is active. */
	abort(): void {
		this.#handle.abort();
	}

	/**
	 * Resolve when the current run and all awaited event listeners have finished.
	 *
	 * This resolves after `agent_end` listeners settle.
	 */
	waitForIdle(): Promise<void> {
		return this.#activeRun?.promise ?? Promise.resolve();
	}

	/** Clear conversation state and queues while retaining the replayed prompt/tool baseline. */
	reset(): void {
		if (this.#activeRun) {
			throw new Error("Agent is already processing. Wait for completion before resetting.");
		}
		this.#sync();
		const baseline: AgentMessage | undefined = this.#handle.reset();
		this.#state.messages = baseline ? [baseline] : [];
		this.#synced.messages = this.#state.messages.slice();
		this.#state.isStreaming = false;
		this.#state.streamingMessage = undefined;
		this.#state.pendingToolCalls = new Set<string>();
		this.#state.errorMessage = undefined;
	}

	/** Start a new prompt from text, a single message, or a batch of messages. */
	async prompt(message: AgentMessage | AgentMessage[]): Promise<void>;
	async prompt(input: string, images?: ImageContent[]): Promise<void>;
	async prompt(input: string | AgentMessage | AgentMessage[], images?: ImageContent[]): Promise<void> {
		if (this.#activeRun) {
			throw new Error(
				"Agent is already processing a prompt. Use steer() or followUp() to queue messages, or wait for completion.",
			);
		}
		this.#sync();
		await this.#run(this.#handle.prompt(input, images, this.#runOptions(), this.#events));
	}

	/** Continue from the current transcript. The last message must be a user or tool-result message. */
	async continue(): Promise<void> {
		if (this.#activeRun) {
			throw new Error("Agent is already processing. Wait for completion before continuing.");
		}
		this.#sync();
		await this.#run(this.#handle.continue(this.#runOptions(), this.#events));
	}

	/** Sends the state's changes since the last run to the Rust agent. */
	#sync(): void {
		const state = this.#state;
		const synced = this.#synced;
		if (state.model !== synced.model) {
			this.#handle.setModel(state.model);
			synced.model = state.model;
		}
		if (state.thinkingLevel !== synced.thinkingLevel) {
			this.#handle.setThinkingLevel(state.thinkingLevel);
			synced.thinkingLevel = state.thinkingLevel;
		}
		const messages = state.messages;
		const known = synced.messages;
		let start = 0;
		while (start < messages.length && start < known.length && messages[start] === known[start]) start++;
		if (start < messages.length || start < known.length) {
			this.#handle.spliceMessages(start, known.length - start, messages.slice(start));
			synced.messages = messages.slice();
		}
	}

	/** What a run reads from the agent when it starts. */
	#runOptions(): Record<string, unknown> {
		return {
			messages: this.#synced.messages,
			model: this.#state.model,
			tools: this.#state.tools,
			streamFn: this.streamFunction,
			convertToLlm: this.convertToLlm === defaultConvertToLlm ? undefined : this.convertToLlm,
			transformContext: this.transformContext,
			getApiKey: this.getApiKey,
			onPayload: this.onPayload,
			onResponse: this.onResponse,
			onProviderStreamEvent: this.onProviderStreamEvent,
			beforeToolCall: this.beforeToolCall,
			afterToolCall: this.afterToolCall,
			finishTurn: this.finishTurn,
			prepareRequest: this.prepareRequest,
			prepareNextTurn: this.prepareNextTurn,
			prepareNextTurnWithContext: this.prepareNextTurnWithContext,
			sessionId: this.sessionId,
			thinkingBudgets: this.thinkingBudgets,
			transport: this.transport,
			maxRetryDelayMs: this.maxRetryDelayMs,
			toolExecution: this.toolExecution,
			identity: this.identity,
		};
	}

	/** Tracks a run the core started; a call it rejected before starting one is returned as is. */
	async #run(run: Promise<void>): Promise<void> {
		if (!this.#handle.isStreaming()) return run;
		let resolvePromise = () => {};
		const promise = new Promise<void>((resolve) => {
			resolvePromise = resolve;
		});
		this.#activeRun = { promise, resolve: resolvePromise };
		this.#state.isStreaming = true;
		this.#state.streamingMessage = undefined;
		this.#state.errorMessage = undefined;
		try {
			await run;
		} finally {
			this.#finishRun();
		}
	}

	#finishRun(): void {
		this.#state.isStreaming = false;
		this.#state.streamingMessage = undefined;
		this.#state.pendingToolCalls = new Set<string>();
		this.#activeRun?.resolve();
		this.#activeRun = undefined;
	}

	/**
	 * Reduce internal state for a loop event, then await listeners.
	 *
	 * `agent_end` only means no further loop events will be emitted. The run is
	 * considered idle later, after all awaited listeners for `agent_end` finish
	 * and `finishRun()` clears runtime-owned state.
	 */
	async #processEvents(event: AgentEvent): Promise<void> {
		switch (event.type) {
			case "message_start":
				this.#state.streamingMessage = event.message;
				break;

			case "message_update":
				this.#state.streamingMessage = event.message;
				break;

			case "message_end":
				this.#state.streamingMessage = undefined;
				this.#state.messages.push(event.message);
				this.#synced.messages.push(event.message);
				break;

			case "tool_execution_start": {
				const pendingToolCalls = new Set(this.#state.pendingToolCalls);
				pendingToolCalls.add(event.toolCallId);
				this.#state.pendingToolCalls = pendingToolCalls;
				break;
			}

			case "tool_execution_end": {
				const pendingToolCalls = new Set(this.#state.pendingToolCalls);
				pendingToolCalls.delete(event.toolCallId);
				this.#state.pendingToolCalls = pendingToolCalls;
				break;
			}

			case "turn_end":
				if (event.message.role === "assistant" && event.message.errorMessage) {
					this.#state.errorMessage = event.message.errorMessage;
				}
				break;

			case "agent_end":
				this.#state.streamingMessage = undefined;
				break;
		}

		const signal = this.signal;
		if (!signal) {
			throw new Error("Agent listener invoked outside active run");
		}
		for (const listener of this.#listeners) {
			await listener(event, signal);
		}
	}
}

/**
 * Stateful wrapper around the low-level agent loop.
 *
 * `Agent` owns the current transcript, emits lifecycle events, executes tools,
 * and exposes queueing APIs for steering and follow-up messages.
 */
export const Agent = FacadeAgent as unknown as typeof ts.Agent;
export type Agent = ts.Agent;
