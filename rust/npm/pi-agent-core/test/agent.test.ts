// Facade tests for behavior the TS package's tests do not cover: the message objects the caller passes to `Agent`.

import type { AssistantMessage, Model, UserMessage } from "@at-inc/pi-ai";
import { createAssistantMessageEventStream } from "@at-inc/pi-ai";
import { describe, expect, it } from "vitest";
import { Agent, type AgentMessage, type StreamFn } from "../src/index.ts";

const model: Model<"openai-responses"> = {
	id: "mock",
	name: "mock",
	api: "openai-responses",
	provider: "openai",
	baseUrl: "https://example.invalid",
	reasoning: false,
	input: ["text"],
	cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0 },
	contextWindow: 1000,
	maxTokens: 100,
};

function user(text: string): UserMessage {
	return { role: "user", content: text, timestamp: Date.now() };
}

function assistant(text: string): AssistantMessage {
	return {
		role: "assistant",
		content: [{ type: "text", text }],
		api: "openai-responses",
		provider: "openai",
		model: "mock",
		usage: {
			input: 0,
			output: 0,
			cacheRead: 0,
			cacheWrite: 0,
			totalTokens: 0,
			cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0, total: 0 },
		},
		stopReason: "stop",
		timestamp: Date.now(),
	};
}

function reply(text: string): ReturnType<StreamFn> {
	const stream = createAssistantMessageEventStream();
	queueMicrotask(() => stream.push({ type: "done", reason: "stop", message: assistant(text) }));
	return stream;
}

function endedMessages(agent: Agent): AgentMessage[] {
	const ended: AgentMessage[] = [];
	agent.subscribe((event) => {
		if (event.type === "message_end") ended.push(event.message);
	});
	return ended;
}

describe("Agent message objects", () => {
	it("emits and keeps the caller's prompt, steering and follow-up objects", async () => {
		const prompt = user("prompt");
		const steering = user("steering");
		const followUp = user("follow-up");
		let requests = 0;
		const agent = new Agent({
			initialState: { model },
			streamFn: () => {
				if (requests++ === 0) {
					agent.steer(steering);
					agent.followUp(followUp);
				}
				return reply(`reply ${requests}`);
			},
		});
		const ended = endedMessages(agent);

		await agent.prompt(prompt);

		expect(requests).toBe(3);
		for (const message of [prompt, steering, followUp]) {
			expect(ended).toContain(message);
			expect(agent.state.messages).toContain(message);
		}
	});

	it("emits the caller's object for a message queued between runs", async () => {
		const followUp = user("queued before the run");
		const agent = new Agent({ initialState: { model }, streamFn: () => reply("ok") });
		const ended = endedMessages(agent);

		agent.followUp(followUp);
		await agent.prompt([user("prompt")]);

		expect(ended).toContain(followUp);
	});

	it("passes the state's message objects to the stream function", async () => {
		const prompt = user("prompt");
		let seen: unknown[] = [];
		const agent = new Agent({
			initialState: { model, messages: [user("earlier")] },
			streamFn: (_model, context) => {
				seen = context.messages;
				return reply("ok");
			},
		});

		await agent.prompt(prompt);

		expect(seen).toEqual(agent.state.messages.slice(0, 2));
		expect(seen[0]).toBe(agent.state.messages[0]);
		expect(seen[1]).toBe(prompt);
	});
});
