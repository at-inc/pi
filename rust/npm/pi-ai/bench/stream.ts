// Compares the facade with the TS package on the same local Chat Completions stream: 2,000 text deltas, then one tool
// call whose arguments are 100 KB in 100 deltas. Each run consumes every event, reads the tool call's arguments at
// toolcall_end and awaits the result. The server sends a prebuilt body, so both sides pay the same server cost.
//
// Run: npm run bench (node --conditions=source bench/stream.ts), after npm run build:wasm in rust/npm

import { createServer } from "node:http";
import type { AssistantMessageEventStream, Model } from "@at-inc/pi-ai";
import { stream as tsStream } from "../../../../packages/ai/src/api/openai-completions.ts";
import { normalizeContext as tsNormalizeContext } from "../../../../packages/ai/src/utils/transcript.ts";
import { stream as rsStream } from "../src/api/openai-completions.ts";
import { normalizeContext as rsNormalizeContext } from "../src/utils/transcript.ts";

const TEXT_DELTAS = 2000;
const ARGUMENT_DELTAS = 100;
const WARMUP = 5;
const RUNS = 30;

const body = buildBody();
const server = createServer((_request, response) => {
	response.writeHead(200, { "content-type": "text/event-stream" });
	response.end(body);
});
await new Promise<void>((resolve) => server.listen(0, "127.0.0.1", resolve));
const { port } = server.address() as { port: number };
const model: Model<"openai-completions"> = {
	id: "bench",
	name: "bench",
	api: "openai-completions",
	provider: "bench",
	baseUrl: `http://127.0.0.1:${port}/v1`,
	reasoning: false,
	input: ["text"],
	cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0 },
	contextWindow: 1_000_000,
	maxTokens: 1000,
};
const messages = [{ role: "user" as const, content: "hi", timestamp: 0 }];

const results = {
	ts: await measure(() => tsStream(model, tsNormalizeContext({ messages }), { apiKey: "bench" })),
	rust: await measure(() => rsStream(model, rsNormalizeContext({ messages }), { apiKey: "bench" })),
};
server.close();

console.log(
	`${TEXT_DELTAS} text deltas + ${ARGUMENT_DELTAS} tool call deltas (${(body.length / 1024).toFixed(0)} KB SSE body), ${RUNS} runs`,
);
for (const [name, times] of Object.entries(results)) {
	times.sort((a, b) => a - b);
	const mean = times.reduce((sum, time) => sum + time, 0) / times.length;
	const median = times[Math.floor(times.length / 2)]!;
	console.log(
		`${name.padEnd(5)} mean ${mean.toFixed(2)} ms  median ${median.toFixed(2)} ms  min ${times[0]!.toFixed(2)} ms`,
	);
}

/** Runs `start` WARMUP + RUNS times and returns the RUNS durations in ms. */
async function measure(start: () => AssistantMessageEventStream): Promise<number[]> {
	const times: number[] = [];
	for (let run = 0; run < WARMUP + RUNS; run++) {
		const startedAt = performance.now();
		const events = start();
		let count = 0;
		let argumentKeys = 0;
		for await (const event of events) {
			count++;
			if (event.type === "toolcall_end") argumentKeys = Object.keys(event.toolCall.arguments).length;
		}
		const message = await events.result();
		if (message.stopReason !== "toolUse" || argumentKeys !== 1 || count < TEXT_DELTAS + ARGUMENT_DELTAS) {
			throw new Error(`unexpected stream: ${message.stopReason} ${message.errorMessage ?? ""} ${count} events`);
		}
		if (run >= WARMUP) times.push(performance.now() - startedAt);
	}
	return times;
}

/** The SSE body: TEXT_DELTAS content chunks, then a tool call with 100 KB of arguments in ARGUMENT_DELTAS chunks. */
function buildBody(): string {
	const chunk = (delta: object, finish: string | null = null) =>
		`data: ${JSON.stringify({ id: "bench", model: "bench", choices: [{ index: 0, delta, finish_reason: finish }] })}\n\n`;
	const parts: string[] = [];
	for (let i = 0; i < TEXT_DELTAS; i++) parts.push(chunk({ content: `word${i} ` }));
	const argumentsJson = JSON.stringify({ text: "x".repeat(100 * 1024 - 11) });
	const size = Math.ceil(argumentsJson.length / ARGUMENT_DELTAS);
	for (let i = 0; i < ARGUMENT_DELTAS; i++) {
		const piece = argumentsJson.slice(i * size, (i + 1) * size);
		const call =
			i === 0
				? { index: 0, id: "call_1", type: "function", function: { name: "write", arguments: piece } }
				: { index: 0, function: { arguments: piece } };
		parts.push(chunk({ tool_calls: [call] }));
	}
	parts.push(chunk({}, "tool_calls"));
	parts.push("data: [DONE]\n\n");
	return parts.join("");
}
