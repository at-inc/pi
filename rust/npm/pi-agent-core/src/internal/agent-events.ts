// The JS side of agent events: receives a Rust run's events as changes (see rust/crates/pi-wasm/src/agent/events.rs)
// and hands each one to the facade as the TS event object.

import type { AgentEvent, AgentMessage } from "@at-inc/pi-agent-core";
import type { AssistantMessage, AssistantMessageEvent, ToolResultMessage } from "@at-inc/pi-ai";
import { AssistantMessageEventSink, syncMessage } from "@at-inc/pi-core";
import type { AgentEventSink } from "@at-inc/pi-core/wasm";

const AGENT_START = 0;
const AGENT_END = 1;
const TURN_START = 2;
const TURN_END = 3;
const MESSAGE_START = 4;
const MESSAGE_END = 5;
const TOOL_EXECUTION_START = 6;
const TOOL_EXECUTION_UPDATE = 7;
const TOOL_EXECUTION_END = 8;

/** A message of `turn_end`/`agent_end`: the index of one of the run's `message_end` messages, or the message. */
type Ref<M> = number | M;

/**
 * Decodes a run's events and passes each to `dispatch`, returning its completion to the core. The streamed response
 * is one live partial (`updates`); its `message_end` message is that partial, synced with the final message, as in
 * the TS, where the final message is the object the stream built.
 */
export class AgentEvents implements AgentEventSink {
	readonly updates: AssistantMessageEventSink;
	readonly #dispatch: (event: AgentEvent) => Promise<void> | void;
	#update: AssistantMessageEvent | undefined;
	#partial: AssistantMessage | undefined;
	#ended: AgentMessage[] = [];
	readonly #toolArgs = new Map<string, unknown>();

	constructor(dispatch: (event: AgentEvent) => Promise<void> | void) {
		this.#dispatch = dispatch;
		this.updates = new AssistantMessageEventSink({
			push: (event) => {
				this.#update = event;
				if ("partial" in event) this.#partial = event.partial;
			},
			end: () => {},
		});
	}

	emit(kind: number, data: string | undefined, message: unknown): Promise<void> | void {
		return this.#dispatch(this.#decode(kind, data, message as AgentMessage | undefined));
	}

	get ended(): AgentMessage | undefined {
		return this.#ended.at(-1);
	}

	update(): Promise<void> | void {
		const event = this.#update!;
		this.#update = undefined;
		const partial = (event as { partial: AssistantMessage }).partial;
		return this.#dispatch({ type: "message_update", assistantMessageEvent: event, message: { ...partial } });
	}

	#decode(kind: number, data: string | undefined, known: AgentMessage | undefined): AgentEvent {
		switch (kind) {
			case AGENT_START:
				this.#ended = [];
				return { type: "agent_start" };
			case AGENT_END: {
				const messages: Ref<AgentMessage>[] = JSON.parse(data!);
				return { type: "agent_end", messages: messages.map((message) => this.#resolve(message)) };
			}
			case TURN_START:
				return { type: "turn_start" };
			case TURN_END: {
				const [message, toolResults]: [Ref<AgentMessage>, Ref<ToolResultMessage>[]] = JSON.parse(data!);
				return {
					type: "turn_end",
					message: this.#resolve(message),
					toolResults: toolResults.map((result) => this.#resolve(result) as ToolResultMessage),
				};
			}
			case MESSAGE_START:
				return { type: "message_start", message: known ?? JSON.parse(data!) };
			case MESSAGE_END: {
				let message: AgentMessage = known ?? JSON.parse(data!);
				if (!known && this.#partial && message.role === "assistant") message = syncMessage(this.#partial, message);
				this.#partial = undefined;
				this.#ended.push(message);
				return { type: "message_end", message };
			}
			case TOOL_EXECUTION_START: {
				const event = JSON.parse(data!);
				this.#toolArgs.set(event.toolCallId, event.args);
				return event;
			}
			case TOOL_EXECUTION_UPDATE: {
				const event = JSON.parse(data!);
				if (!("args" in event)) event.args = this.#toolArgs.get(event.toolCallId);
				return event;
			}
			case TOOL_EXECUTION_END: {
				const event = JSON.parse(data!);
				this.#toolArgs.delete(event.toolCallId);
				return event;
			}
		}
		throw new Error(`Unknown agent event kind ${kind}`);
	}

	#resolve<M>(message: Ref<M>): M {
		return typeof message === "number" ? (this.#ended[message] as M) : message;
	}
}
