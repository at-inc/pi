// The facade of agent-loop.ts. `runAgentLoop`, `runAgentLoopContinue` and `runToolCall` run in the core (see
// rust/crates/pi-wasm/src/agent/agent_loop.rs); `agentLoop` and `agentLoopContinue` are built on them as in the TS.

import type * as ts from "@at-inc/pi-agent-core";
import type { AgentEvent, AgentMessage } from "@at-inc/pi-agent-core";
import { EventStream } from "@at-inc/pi-ai-rs";
import { wasm } from "@at-inc/pi-core";
import { AgentEvents } from "./internal/agent-events.ts";

export type { AgentEventSink, RunToolCallOptions, ToolCallHooks } from "@at-inc/pi-agent-core";

/**
 * Request identity of agent loops: pi-ai's defaults with provider attribution to pi. `AgentLoopConfig.identity`
 * overrides it field by field.
 */
export const DEFAULT_AGENT_IDENTITY: typeof ts.DEFAULT_AGENT_IDENTITY = wasm.agentDefaultAgentIdentity();

/**
 * Start an agent loop with a new prompt message.
 * The prompt is added to the context and events are emitted for it.
 */
export const agentLoop: typeof ts.agentLoop = (prompts, context, config, signal, streamFn) => {
	const stream = createAgentStream();
	void runAgentLoop(
		prompts,
		context,
		config,
		async (event) => {
			stream.push(event);
		},
		signal,
		streamFn,
	).then((messages) => {
		stream.end(messages);
	});
	return stream;
};

/**
 * Continue an agent loop from the current context without adding a new message.
 * Used for retries - context already has user message or tool results.
 *
 * **Important:** The last message in context must convert to a `user` or `toolResult` message
 * via `convertToLlm`. If it doesn't, the LLM provider will reject the request.
 * This cannot be validated here since `convertToLlm` is only called once per turn.
 */
export const agentLoopContinue: typeof ts.agentLoopContinue = (context, config, signal, streamFn) => {
	if (context.messages.length === 0) {
		throw new Error("Cannot continue: no messages in context");
	}
	if (context.messages[context.messages.length - 1].role === "assistant") {
		throw new Error("Cannot continue from message role: assistant");
	}
	const stream = createAgentStream();
	void runAgentLoopContinue(
		context,
		config,
		async (event) => {
			stream.push(event);
		},
		signal,
		streamFn,
	).then((messages) => {
		stream.end(messages);
	});
	return stream;
};

export const runAgentLoop: typeof ts.runAgentLoop = (prompts, context, config, emit, signal, streamFn) =>
	wasm.agentRunAgentLoop(prompts, context, config, new AgentEvents(emit), signal, streamFn);

export const runAgentLoopContinue: typeof ts.runAgentLoopContinue = (context, config, emit, signal, streamFn) =>
	wasm.agentRunAgentLoopContinue(context, config, new AgentEvents(emit), signal, streamFn);

/**
 * Run one tool call through the same steps as a model-issued call: argument preparation, schema
 * validation, `beforeToolCall`, execution, and `afterToolCall`. Emits no events and adds no
 * messages. Tools that call other tools use this so the hooks (for example permission checks)
 * apply to those calls too.
 *
 * Never rejects for tool failures: unknown tools, validation errors, blocked calls, and thrown
 * errors come back as `isError: true`.
 */
export const runToolCall: typeof ts.runToolCall = (toolCall, options) => wasm.agentRunToolCall(toolCall, options);

function createAgentStream(): EventStream<AgentEvent, AgentMessage[]> {
	return new EventStream<AgentEvent, AgentMessage[]>(
		(event: AgentEvent) => event.type === "agent_end",
		(event: AgentEvent) => (event.type === "agent_end" ? event.messages : []),
	);
}
