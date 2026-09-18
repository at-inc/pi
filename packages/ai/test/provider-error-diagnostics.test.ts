import { describe, expect, it } from "vitest";
import { stream as streamAnthropic } from "../src/api/anthropic-messages.ts";
import { stream as streamCodex } from "../src/api/openai-codex-responses.ts";
import { stream as streamCompletions } from "../src/api/openai-completions.ts";
import { stream as streamResponses } from "../src/api/openai-responses.ts";
import type { Api, Model, StreamOptions } from "../src/types.ts";
import { createProviderErrorDiagnostic } from "../src/utils/diagnostics.ts";
import { normalizeContext } from "../src/utils/transcript.ts";

const context = normalizeContext({ messages: [{ role: "user", content: "Hello", timestamp: 0 }] });
const token = `test.${Buffer.from(JSON.stringify({ "https://api.openai.com/auth": { chatgpt_account_id: "test" } })).toString("base64url")}.test`;

function model<T extends Api>(api: T): Model<T> {
	return {
		id: "test-model",
		name: "Test",
		api,
		provider: "test",
		baseUrl: "https://provider.invalid/v1",
		reasoning: false,
		input: ["text"],
		cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0 },
		contextWindow: 10000,
		maxTokens: 1000,
	};
}

const providers = [
	{
		name: "anthropic",
		stream: (options: StreamOptions) => streamAnthropic(model("anthropic-messages"), context, options),
	},
	{
		name: "completions",
		stream: (options: StreamOptions) => streamCompletions(model("openai-completions"), context, options),
	},
	{
		name: "responses",
		stream: (options: StreamOptions) => streamResponses(model("openai-responses"), context, options),
	},
	{
		name: "codex",
		stream: (options: StreamOptions) => streamCodex(model("openai-codex-responses"), context, options),
	},
];

describe("provider error diagnostics", () => {
	it.each(providers)("$name preserves HTTP metadata through JSON serialization", async ({ stream }) => {
		const error = {
			code: "usage_limit_reached",
			type: "rate_limit_error",
			message: "Plan limit",
			plan_type: "pro",
			resets_at: 2000000000,
		};
		const output = await stream({
			apiKey: token,
			transport: "sse",
			maxRetries: 0,
			fetch: async () =>
				Response.json({ error }, { status: 429, headers: { "retry-after": "60", "x-request-id": "test-request" } }),
		}).result();
		expect(output.stopReason).toBe("error");
		const diagnostic = JSON.parse(JSON.stringify(output.diagnostics?.find((item) => item.type === "provider_error")));
		expect(diagnostic.details).toMatchObject({
			status: 429,
			headers: { "retry-after": "60", "x-request-id": "test-request" },
		});
		// SDKs differ on whether .error includes the outer error envelope.
		expect(diagnostic.details.payload.error ?? diagnostic.details.payload).toEqual(error);
	});

	it.each(providers)("$name does not classify cancellation as a provider failure", async ({ stream }) => {
		const controller = new AbortController();
		controller.abort();
		const output = await stream({
			apiKey: token,
			transport: "sse",
			maxRetries: 0,
			signal: controller.signal,
			fetch: async () => {
				throw new Error("Request was aborted");
			},
		}).result();
		expect(output.stopReason).toBe("aborted");
		expect(output.diagnostics?.some((item) => item.type === "provider_error")).not.toBe(true);
	});

	it("preserves Codex stream error events", async () => {
		const event = {
			type: "error",
			code: "usage_limit_reached",
			message: "Plan limit",
			plan_type: "pro",
			resets_at: 2000000000,
		};
		const output = await streamCodex(model("openai-codex-responses"), context, {
			apiKey: token,
			transport: "sse",
			fetch: async () =>
				new Response(`data: ${JSON.stringify(event)}\n\n`, { headers: { "content-type": "text/event-stream" } }),
		}).result();
		expect(output.diagnostics).toContainEqual(
			expect.objectContaining({
				type: "provider_error",
				error: expect.objectContaining({ code: "usage_limit_reached" }),
				details: expect.objectContaining({ payload: event }),
			}),
		);
	});

	it("accepts statusCode and plain header records", () => {
		const payload = { message: "Plan limit" };
		expect(
			createProviderErrorDiagnostic({ statusCode: 403, headers: { "x-request-id": "request" }, payload }).details,
		).toEqual({
			status: 403,
			headers: { "x-request-id": "request" },
			payload,
		});
		expect(createProviderErrorDiagnostic(null).type).toBe("provider_error");
	});

	it.each(["commentary", "final_answer"])("retains %s identity before output_item.done", async (phase) => {
		const events = [
			{
				type: "response.output_item.added",
				output_index: 0,
				item: { type: "message", id: "msg_test", role: "assistant", status: "in_progress", phase, content: [] },
			},
			{
				type: "response.output_text.delta",
				output_index: 0,
				content_index: 0,
				item_id: "msg_test",
				delta: "Partial answer",
			},
		];
		const stream = streamResponses(model("openai-responses"), context, {
			apiKey: "test",
			fetch: async () =>
				new Response(events.map((event) => `data: ${JSON.stringify(event)}\n\n`).join(""), {
					headers: { "content-type": "text/event-stream" },
				}),
		});
		let startingSignature: string | undefined;
		for await (const event of stream) {
			if (event.type === "text_start") {
				const block = event.partial.content[event.contentIndex];
				if (block.type === "text") startingSignature = block.textSignature;
			}
		}
		const output = await stream.result();
		expect(output.stopReason).toBe("error");
		expect(startingSignature).toBe(JSON.stringify({ v: 1, id: "msg_test", phase }));
		expect(output.content).toEqual([{ type: "text", text: "Partial answer", textSignature: startingSignature }]);
	});
});
