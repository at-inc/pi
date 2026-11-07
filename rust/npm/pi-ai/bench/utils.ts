// Boundary cost of the bridged utils: the facade against the TS package on the same inputs. The context has 200
// messages (user text, assistant text with a tool call and usage, tool results) and a system prompt with two tools.
// Each case prints the median of RUNS calls after WARMUP calls, in milliseconds.
//
// Run: node --conditions=source bench/utils.ts in rust/npm/pi-ai, after npm run build:wasm in rust/npm

import type { AssistantMessage, Context, Message, Tool, ToolCall } from "@at-inc/pi-ai";
import * as tsJson from "../../../../packages/ai/src/utils/json-parse.ts";
import * as tsEstimate from "../../../../packages/ai/src/utils/estimate.ts";
import * as tsOverflow from "../../../../packages/ai/src/utils/overflow.ts";
import * as tsText from "../../../../packages/ai/src/utils/text.ts";
import * as tsTranscript from "../../../../packages/ai/src/utils/transcript.ts";
import * as tsValidation from "../../../../packages/ai/src/utils/validation.ts";
import * as rsJson from "../src/utils/json-parse.ts";
import * as rsEstimate from "../src/utils/estimate.ts";
import * as rsOverflow from "../src/utils/overflow.ts";
import * as rsText from "../src/utils/text.ts";
import * as rsTranscript from "../src/utils/transcript.ts";
import * as rsValidation from "../src/utils/validation.ts";

const MESSAGES = 200;
const WARMUP = 50;
const RUNS = 300;

const tool: Tool = {
	name: "edit",
	description: "Edit a file",
	parameters: {
		type: "object",
		properties: { path: { type: "string" }, edits: { type: "array", items: { type: "string" } } },
		required: ["path", "edits"],
	},
};
const assistant = (index: number): AssistantMessage => ({
	role: "assistant",
	content: [
		{ type: "text", text: `Reply ${index}: ${"lorem ipsum ".repeat(40)}` },
		{ type: "toolCall", id: `call-${index}`, name: "edit", arguments: { path: `src/${index}.ts`, edits: ["a", "b"] } },
	],
	api: "openai-completions",
	provider: "bench",
	model: "bench",
	usage: {
		input: 1000 + index,
		output: 200,
		cacheRead: 0,
		cacheWrite: 0,
		totalTokens: 1200 + index,
		cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0, total: 0 },
	},
	stopReason: "toolUse",
	timestamp: index,
});
const messages: Message[] = [];
for (let index = 0; messages.length < MESSAGES; index++) {
	messages.push({ role: "user", content: `Question ${index}: ${"dolor sit amet ".repeat(30)}`, timestamp: index });
	messages.push(assistant(index));
	messages.push({
		role: "toolResult",
		toolCallId: `call-${index}`,
		toolName: "edit",
		content: [{ type: "text", text: "ok ".repeat(50) }],
		isError: false,
		timestamp: index,
	});
}
messages.length = MESSAGES;
const context: Context = { systemPrompt: "You are a bench.", tools: [tool, { ...tool, name: "read" }], messages };
const transcript = tsTranscript.normalizeContext(context);
const last = assistant(MESSAGES);
const toolCall: ToolCall = { type: "toolCall", id: "c", name: "edit", arguments: { path: "a.ts", edits: ["x", "y"] } };
const argumentsText = JSON.stringify({ path: "src/index.ts", edits: Array.from({ length: 50 }, (_, i) => `edit ${i}`) });

const cases: [string, () => unknown, () => unknown][] = [
	["estimateContextTokens", () => tsEstimate.estimateContextTokens(transcript), () => rsEstimate.estimateContextTokens(transcript)],
	["normalizeContext", () => tsTranscript.normalizeContext(context), () => rsTranscript.normalizeContext(context)],
	["getCurrentTools", () => tsTranscript.getCurrentTools(transcript.messages), () => rsTranscript.getCurrentTools(transcript.messages)],
	["estimateMessageTokens", () => tsEstimate.estimateMessageTokens(last), () => rsEstimate.estimateMessageTokens(last)],
	["contentText", () => tsText.contentText(last.content), () => rsText.contentText(last.content)],
	["isContextOverflow", () => tsOverflow.isContextOverflow(last, 200_000), () => rsOverflow.isContextOverflow(last, 200_000)],
	["parseStreamingJson (1 KB)", () => tsJson.parseStreamingJson(argumentsText.slice(0, -5)), () => rsJson.parseStreamingJson(argumentsText.slice(0, -5))],
	["validateToolArguments", () => tsValidation.validateToolArguments(tool, toolCall), () => rsValidation.validateToolArguments(tool, toolCall)],
];

function median(run: () => unknown): number {
	for (let i = 0; i < WARMUP; i++) run();
	const times: number[] = [];
	for (let i = 0; i < RUNS; i++) {
		const started = performance.now();
		run();
		times.push(performance.now() - started);
	}
	times.sort((a, b) => a - b);
	return times[Math.floor(times.length / 2)]!;
}

console.log(`context: ${MESSAGES} messages, ${JSON.stringify(transcript).length} chars of JSON; median of ${RUNS} runs`);
for (const [name, ts, rs] of cases) {
	const tsMs = median(ts);
	const rsMs = median(rs);
	console.log(`${name.padEnd(28)} TS ${tsMs.toFixed(4)} ms  Rust ${rsMs.toFixed(4)} ms  ${(rsMs / tsMs).toFixed(0)}x`);
}
