import { createModels, Type } from "@at-inc/pi-ai";
import { fauxAssistantMessage, fauxProvider, fauxToolCall } from "@at-inc/pi-ai/providers/faux";
import { afterEach, describe, expect, it, vi } from "vitest";
import { Agent } from "../src/agent.ts";
import { type ProxyAssistantMessageEvent, streamProxy } from "../src/proxy.ts";
import type { AgentTool, StreamFn } from "../src/types.ts";

afterEach(() => vi.unstubAllGlobals());

describe("malformed final tool arguments", () => {
	// Regression for https://github.com/earendil-works/pi/issues/10897.
	it.each([
		{ transport: "direct", prepare: false },
		{ transport: "direct", prepare: true },
		{ transport: "proxy", prepare: false },
		{ transport: "proxy", prepare: true },
	])("never executes through $transport with prepareArguments=$prepare", async ({ transport, prepare }) => {
		const call = {
			...fauxToolCall("unsafe", {}, { id: "malformed-call" }),
			argumentsError: 'Invalid JSON object: {"value":',
		};
		const first = fauxAssistantMessage([call], { stopReason: "toolUse" });
		const faux = fauxProvider();
		faux.setResponses(
			transport === "direct" ? [first, fauxAssistantMessage("done")] : [fauxAssistantMessage("done")],
		);
		const models = createModels();
		models.setProvider(faux.provider);
		const execute = vi.fn(async () => ({ content: [{ type: "text" as const, text: "unexpected" }], details: {} }));
		const prepareArguments = vi.fn(() => ({}));
		const parameters = Type.Object({});
		const tool: AgentTool<typeof parameters> = {
			name: "unsafe",
			label: "Unsafe",
			description: "Must not execute malformed arguments",
			parameters,
			execute,
			...(prepare ? { prepareArguments } : {}),
		};
		let requests = 0;
		const streamFn: StreamFn = (model, context, options) => {
			requests++;
			if (transport === "direct" || requests > 1) return models.streamSimple(model, context, options);
			const events: ProxyAssistantMessageEvent[] = [
				{ type: "start" },
				{ type: "toolcall_start", contentIndex: 0, id: call.id, toolName: call.name },
				{ type: "toolcall_delta", contentIndex: 0, delta: '{"value":' },
				{ type: "toolcall_end", contentIndex: 0, toolCall: call },
				{ type: "done", reason: "toolUse", usage: first.usage },
			];
			vi.stubGlobal(
				"fetch",
				vi.fn(async () => new Response(events.map((event) => `data: ${JSON.stringify(event)}\n\n`).join(""))),
			);
			return streamProxy(model, context, { authToken: "test-token", proxyUrl: "https://proxy.example" });
		};
		const beforeToolCall = vi.fn();
		const agent = new Agent({ initialState: { model: faux.getModel(), tools: [tool] }, streamFn, beforeToolCall });
		await agent.prompt("Attempt the tool call.");

		expect(execute).not.toHaveBeenCalled();
		expect(beforeToolCall).not.toHaveBeenCalled();
		expect(prepareArguments).toHaveBeenCalledTimes(prepare ? 1 : 0);
		expect(requests).toBe(2);
		expect(agent.state.errorMessage).toBeUndefined();
		expect(agent.state.messages.find((message) => message.role === "toolResult")).toMatchObject({
			isError: true,
			toolCallId: call.id,
			content: [{ type: "text", text: expect.stringContaining(call.argumentsError) }],
		});
	});
});
