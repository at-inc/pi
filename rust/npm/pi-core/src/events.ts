// The JS side of event streams: the TS event stream classes, and the sink that receives a Rust stream's events (see
// rust/crates/pi-wasm/src/events.rs) and keeps one live JS partial message per stream.

import type {
	AssistantMessage,
	AssistantMessageEvent,
	StopReason,
	TextContent,
	ThinkingContent,
	ToolCall,
} from "@at-inc/pi-ai";
import type * as ts from "@at-inc/pi-ai/utils/event-stream";
import type { AssistantMessageHandle, AssistantMessageEventSink as WasmEventSink } from "../wasm/pi_wasm.js";

class FifoQueue<T> {
	private incoming: T[] = [];
	private outgoing: T[] = [];

	get length(): number {
		return this.incoming.length + this.outgoing.length;
	}

	enqueue(value: T): void {
		this.incoming.push(value);
	}

	dequeue(): T | undefined {
		if (this.outgoing.length === 0) {
			while (this.incoming.length > 0) {
				this.outgoing.push(this.incoming.pop()!);
			}
		}
		return this.outgoing.pop();
	}
}

// The TS package's `EventStream`, line for line.
class CoreEventStream<T, R = T> implements AsyncIterable<T> {
	private queue = new FifoQueue<T>();
	private waiting = new FifoQueue<(value: IteratorResult<T>) => void>();
	protected done = false;
	private finalResultPromise: Promise<R>;
	private resolveFinalResult!: (result: R) => void;
	private isComplete: (event: T) => boolean;
	private extractResult: (event: T) => R;

	constructor(isComplete: (event: T) => boolean, extractResult: (event: T) => R) {
		this.isComplete = isComplete;
		this.extractResult = extractResult;
		this.finalResultPromise = new Promise((resolve) => {
			this.resolveFinalResult = resolve;
		});
	}

	push(event: T): void {
		if (this.done) return;

		if (this.isComplete(event)) {
			this.done = true;
			this.resolveFinalResult(this.extractResult(event));
		}

		const waiter = this.waiting.dequeue();
		if (waiter) {
			waiter({ value: event, done: false });
		} else {
			this.queue.enqueue(event);
		}
	}

	end(result?: R): void {
		this.done = true;
		if (result !== undefined) {
			this.resolveFinalResult(result);
		}
		while (this.waiting.length > 0) {
			const waiter = this.waiting.dequeue()!;
			waiter({ value: undefined as never, done: true });
		}
	}

	async *[Symbol.asyncIterator](): AsyncIterator<T> {
		while (true) {
			if (this.queue.length > 0) {
				yield this.queue.dequeue()!;
			} else if (this.done) {
				return;
			} else {
				const result = await new Promise<IteratorResult<T>>((resolve) => this.waiting.enqueue(resolve));
				if (result.done) return;
				yield result.value;
			}
		}
	}

	result(): Promise<R> {
		return this.finalResultPromise;
	}
}

// The TS package's `AssistantMessageEventStream`, line for line, including its timing of the final message
// (`durationMs`), which a stream fed by the core finds already set.
class CoreAssistantMessageEventStream extends CoreEventStream<AssistantMessageEvent, AssistantMessage> {
	readonly #startedAt = Date.now();
	readonly #startedAtMonotonic = performance.now();

	constructor() {
		super(
			(event) => event.type === "done" || event.type === "error",
			(event) => {
				if (event.type === "done") {
					return event.message;
				} else if (event.type === "error") {
					return event.error;
				}
				throw new Error("Unexpected event type for final result");
			},
		);
	}

	override push(event: AssistantMessageEvent): void {
		if (event.type === "done") this.#time(event.message);
		else if (event.type === "error") this.#time(event.error);
		super.push(event);
	}

	override end(result?: AssistantMessage): void {
		if (result !== undefined) this.#time(result);
		super.end(result);
	}

	#time(message: AssistantMessage): void {
		if (this.done || message.durationMs !== undefined || message.timestamp < this.#startedAt) return;
		message.durationMs = Math.max(0, Math.round(performance.now() - this.#startedAtMonotonic));
	}
}

// The TS classes have `#private` members, which makes TypeScript compare them by declaration instead of by shape. The
// classes above are the same code, so they are typed as the TS classes and pass wherever the TS types are expected.

/** Generic event stream for async iteration; the TS package's `EventStream`. */
export const EventStream = CoreEventStream as unknown as typeof ts.EventStream;
export type EventStream<T, R = T> = ts.EventStream<T, R>;

/** Event stream of one assistant response; the TS package's `AssistantMessageEventStream`. */
export const AssistantMessageEventStream =
	CoreAssistantMessageEventStream as unknown as typeof ts.AssistantMessageEventStream;
export type AssistantMessageEventStream = ts.AssistantMessageEventStream;

/** Factory function for AssistantMessageEventStream (for use in extensions) */
export function createAssistantMessageEventStream(): AssistantMessageEventStream {
	return new AssistantMessageEventStream();
}

/** Starts a Rust stream with a sink for a new JS stream and returns the JS stream; `start` throws like the TS call. */
export function streamFrom(start: (sink: AssistantMessageEventSink) => void): AssistantMessageEventStream {
	const stream = new CoreAssistantMessageEventStream();
	start(new AssistantMessageEventSink(stream));
	return stream as unknown as AssistantMessageEventStream;
}

const START = 0;
const TEXT_START = 1;
const TEXT_DELTA = 2;
const TEXT_END = 3;
const THINKING_START = 4;
const THINKING_DELTA = 5;
const THINKING_END = 6;
const TOOLCALL_START = 7;
const TOOLCALL_DELTA = 8;
const TOOLCALL_END = 9;
const DONE = 10;
const ERROR = 11;
const END = 12;

type Block = TextContent | ThinkingContent | ToolCall;

/** What an `AssistantMessageEventSink` pushes its events into: a stream, or a facade that handles each event. */
export interface AssistantMessageEventTarget {
	push(event: AssistantMessageEvent): void;
	end(result?: AssistantMessage): void;
}

/**
 * Receives a Rust stream's events and pushes TS events into `stream`. It keeps one live `partial` and applies each
 * event's change to it, so every event's `partial` is the same object, as in the TS. A tool call's `arguments` are read
 * from the core when JS reads them, not on every delta.
 */
export class AssistantMessageEventSink implements WasmEventSink {
	readonly #stream: AssistantMessageEventTarget;
	#partial: AssistantMessage | undefined;
	#handle: AssistantMessageHandle | undefined;
	readonly #staleArguments = new Map<ToolCall, number>();

	constructor(stream: AssistantMessageEventTarget) {
		this.#stream = stream;
	}

	attach(partial: AssistantMessageHandle): void {
		this.#release();
		this.#handle = partial;
	}

	event(
		kind: number,
		index: number,
		text: string | undefined,
		head: string | undefined,
		block: string | undefined,
		message: string | undefined,
	): void {
		if (kind === END) {
			this.#release();
			this.#stream.end(message === undefined ? undefined : JSON.parse(message));
			return;
		}
		if (kind === DONE || kind === ERROR) {
			const final: AssistantMessage = JSON.parse(message!);
			const live = index === 1 && this.#partial ? syncMessage(this.#partial, final) : final;
			this.#release();
			const reason = text as StopReason;
			this.#stream.push(
				kind === DONE
					? { type: "done", reason: reason as "stop", message: live }
					: { type: "error", reason: reason as "error", error: live },
			);
			return;
		}
		if (message !== undefined) {
			this.#staleArguments.clear();
			this.#partial = JSON.parse(message);
		}
		const partial = this.#partial!;
		if (head !== undefined) syncHead(partial, JSON.parse(head));
		if (block !== undefined) setBlock(partial, index, JSON.parse(block));
		const content = partial.content as Block[];
		switch (kind) {
			case START:
				this.#stream.push({ type: "start", partial });
				return;
			case TEXT_START:
				this.#stream.push({ type: "text_start", contentIndex: index, partial });
				return;
			case TEXT_DELTA:
				if (block === undefined) (content[index] as TextContent).text += text;
				this.#stream.push({ type: "text_delta", contentIndex: index, delta: text!, partial });
				return;
			case TEXT_END:
				this.#stream.push({
					type: "text_end",
					contentIndex: index,
					content: (content[index] as TextContent).text,
					partial,
				});
				return;
			case THINKING_START:
				this.#stream.push({ type: "thinking_start", contentIndex: index, partial });
				return;
			case THINKING_DELTA:
				if (block === undefined) (content[index] as ThinkingContent).thinking += text;
				this.#stream.push({ type: "thinking_delta", contentIndex: index, delta: text!, partial });
				return;
			case THINKING_END:
				this.#stream.push({
					type: "thinking_end",
					contentIndex: index,
					content: (content[index] as ThinkingContent).thinking,
					partial,
				});
				return;
			case TOOLCALL_START:
				this.#stream.push({ type: "toolcall_start", contentIndex: index, partial });
				return;
			case TOOLCALL_DELTA:
				this.#markArgumentsStale(content[index], index);
				this.#stream.push({ type: "toolcall_delta", contentIndex: index, delta: text!, partial });
				return;
			case TOOLCALL_END:
				this.#stream.push({
					type: "toolcall_end",
					contentIndex: index,
					toolCall: content[index] as ToolCall,
					partial,
				});
				return;
		}
	}

	/** Makes `block.arguments` read the core's current arguments the next time JS reads it. */
	#markArgumentsStale(block: Block | undefined, index: number): void {
		if (block?.type !== "toolCall") return;
		if (!this.#staleArguments.has(block)) {
			let value = block.arguments;
			Object.defineProperty(block, "arguments", {
				configurable: true,
				enumerable: true,
				get: () => {
					if (this.#staleArguments.get(block) === 1 && this.#handle) {
						const json = this.#handle.toolCallArguments(index);
						value = json === undefined ? {} : JSON.parse(json);
						this.#staleArguments.set(block, 0);
					}
					return value;
				},
				set: (next) => {
					value = next;
					this.#staleArguments.set(block, 0);
				},
			});
		}
		this.#staleArguments.set(block, 1);
	}

	#release(): void {
		this.#handle?.free();
		this.#handle = undefined;
	}
}

/** Replaces the message fields other than `content`. */
function syncHead(target: AssistantMessage, head: AssistantMessage): void {
	const record = target as unknown as Record<string, unknown>;
	for (const key of Object.keys(record)) {
		if (key !== "content" && !(key in head)) delete record[key];
	}
	for (const [key, value] of Object.entries(head)) {
		if (key !== "content") record[key] = value;
	}
}

/** Sets block `index`, keeping an existing block object (events may hold it) and replacing its fields. */
function setBlock(target: AssistantMessage, index: number, block: Block): void {
	const content = target.content as Block[];
	const existing = content[index];
	if (existing === undefined) {
		content[index] = block;
		return;
	}
	const record = existing as unknown as Record<string, unknown>;
	for (const key of Object.keys(record)) {
		if (!(key in block)) delete record[key];
	}
	for (const [key, value] of Object.entries(block)) {
		Object.defineProperty(record, key, { value, writable: true, enumerable: true, configurable: true });
	}
}

/** Makes `target` equal `source`, keeping `target` and its block objects. */
export function syncMessage(target: AssistantMessage, source: AssistantMessage): AssistantMessage {
	syncHead(target, source);
	source.content.forEach((block, index) => {
		setBlock(target, index, block as Block);
	});
	target.content.length = source.content.length;
	return target;
}
