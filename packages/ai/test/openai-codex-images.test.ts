import { zstdDecompressSync } from "node:zlib";
import { describe, expect, it, vi } from "vitest";
import { generateImages } from "../src/api/openai-codex-images.ts";
import { builtinImagesProviders } from "../src/providers/all.ts";
import type { ImagesContext, ImagesModel } from "../src/types.ts";

const MODEL: ImagesModel<"openai-codex-images"> = {
	id: "chatgpt-image-generation",
	name: "ChatGPT Image Generation",
	api: "openai-codex-images",
	provider: "openai-codex",
	baseUrl: "https://chatgpt.com/backend-api",
	input: ["text", "image"],
	output: ["image"],
	cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0 },
};
const IMAGE_DATA = "aW1hZ2U=";

function mockToken(): string {
	const payload = Buffer.from(
		JSON.stringify({ "https://api.openai.com/auth": { chatgpt_account_id: "acc_test" } }),
	).toString("base64");
	return `header.${payload}.signature`;
}

function decodeRequestBody(body: RequestInit["body"] | undefined): Record<string, unknown> {
	if (typeof body === "string") return JSON.parse(body) as Record<string, unknown>;
	if (body instanceof Uint8Array) {
		return JSON.parse(Buffer.from(zstdDecompressSync(body)).toString("utf8")) as Record<string, unknown>;
	}
	throw new Error("Expected a Codex request body");
}

function sse(events: readonly object[]): Response {
	return new Response(`${events.map((event) => `data: ${JSON.stringify(event)}`).join("\n\n")}\n\n`, {
		headers: { "content-type": "text/event-stream" },
	});
}

function completedResponse(output: readonly object[]) {
	return {
		type: "response.completed",
		response: {
			id: "resp_image",
			status: "completed",
			output,
			usage: {
				input_tokens: 5,
				output_tokens: 3,
				total_tokens: 8,
				input_tokens_details: { cached_tokens: 0 },
				output_tokens_details: { reasoning_tokens: 0 },
			},
		},
	};
}

describe("OpenAI Codex images", () => {
	it.each([
		{
			name: "generation with a terminal-only result",
			action: "generate",
			context: { input: [{ type: "text" as const, text: "Draw a fox" }] },
			events: [
				completedResponse([{ type: "image_generation_call", id: "ig_1", status: "completed", result: IMAGE_DATA }]),
			],
		},
		{
			name: "editing with item-done and terminal deduplication",
			action: "edit",
			context: {
				input: [
					{ type: "text" as const, text: "Add a hat" },
					{ type: "image" as const, data: IMAGE_DATA, mimeType: "image/png" },
				],
			},
			events: [
				{
					type: "response.output_item.added",
					output_index: 0,
					item: { type: "image_generation_call", id: "ig_1", status: "in_progress", result: null },
				},
				{
					type: "response.output_item.done",
					output_index: 0,
					item: { type: "image_generation_call", id: "ig_1", status: "completed", result: IMAGE_DATA },
				},
				completedResponse([{ type: "image_generation_call", id: "ig_1", status: "completed", result: IMAGE_DATA }]),
			],
		},
		{
			name: "Codex partial-only output",
			action: "generate",
			context: { input: [{ type: "text" as const, text: "Draw a circle" }] },
			events: [
				{
					type: "response.output_item.added",
					output_index: 0,
					item: { type: "image_generation_call", id: "ig_1", status: "in_progress", result: null },
				},
				{
					type: "response.image_generation_call.partial_image",
					item_id: "ig_1",
					output_index: 0,
					partial_image_index: 0,
					partial_image_b64: IMAGE_DATA,
				},
				{
					type: "response.output_item.done",
					output_index: 0,
					item: { type: "image_generation_call", id: "ig_1", status: "generating", result: null },
				},
				completedResponse([{ type: "image_generation_call", id: "ig_1", status: "completed", result: null }]),
			],
		},
	])("handles $name", async ({ action, context, events }) => {
		let requestBody: Record<string, unknown> | undefined;
		const fetch = vi.fn(async (_input: string | URL | Request, init?: RequestInit) => {
			requestBody = decodeRequestBody(init?.body);
			const headers = new Headers(init?.headers);
			expect(headers.get("authorization")).toBe(`Bearer ${mockToken()}`);
			expect(headers.get("chatgpt-account-id")).toBe("acc_test");
			expect(init?.signal).toBeInstanceOf(AbortSignal);
			return sse(events);
		});

		const result = await generateImages(MODEL, context satisfies ImagesContext, {
			apiKey: mockToken(),
			fetch,
			maxRetries: 9,
		});

		expect(result).toMatchObject({
			responseId: "resp_image",
			stopReason: "stop",
			output: [{ type: "image", data: IMAGE_DATA, mimeType: "image/png" }],
		});
		expect(requestBody).toMatchObject({
			model: "gpt-5.4-mini",
			store: false,
			stream: true,
			tools: [{ type: "image_generation", action }],
			tool_choice: { type: "image_generation" },
			parallel_tool_calls: false,
		});
		expect(requestBody).not.toHaveProperty("previous_response_id");
		expect(requestBody).not.toHaveProperty("prompt_cache_key");
		if (action === "edit") {
			expect(requestBody).toMatchObject({
				input: [
					{
						role: "user",
						content: expect.arrayContaining([
							expect.objectContaining({
								type: "input_image",
								image_url: `data:image/png;base64,${IMAGE_DATA}`,
							}),
						]),
					},
				],
			});
		}
		expect(fetch).toHaveBeenCalledOnce();
	});

	it("handles provider failures without retries and honors in-flight cancellation", async () => {
		const malformed = await generateImages(
			MODEL,
			{ input: [{ type: "text", text: "Draw" }] },
			{
				apiKey: mockToken(),
				fetch: async () =>
					sse([
						completedResponse([
							{ type: "image_generation_call", id: "ig_1", status: "completed", result: "not base64" },
						]),
					]),
			},
		);
		expect(malformed).toMatchObject({ stopReason: "error", errorMessage: "ChatGPT returned malformed image data" });

		const failed = await generateImages(
			MODEL,
			{ input: [{ type: "text", text: "Draw" }] },
			{
				apiKey: mockToken(),
				fetch: async () =>
					sse([
						completedResponse([{ type: "image_generation_call", id: "ig_1", status: "failed", result: null }]),
					]),
			},
		);
		expect(failed).toMatchObject({ stopReason: "error", errorMessage: "ChatGPT image generation failed" });

		const failedWithAssistantText = await generateImages(
			MODEL,
			{ input: [{ type: "text", text: "Draw" }] },
			{
				apiKey: mockToken(),
				fetch: async () =>
					sse([
						{
							type: "response.output_item.added",
							output_index: 0,
							item: { type: "message", id: "msg_1", role: "assistant", status: "in_progress", content: [] },
						},
						{ type: "response.content_part.added", part: { type: "output_text", text: "" } },
						{ type: "response.output_text.delta", output_index: 0, delta: "Policy violation: cannot generate this image." },
						{
							type: "response.output_item.done",
							output_index: 0,
							item: {
								type: "message",
								id: "msg_1",
								role: "assistant",
								status: "completed",
								content: [{ type: "output_text", text: "Policy violation: cannot generate this image." }],
							},
						},
						completedResponse([{ type: "image_generation_call", id: "ig_1", status: "failed", result: null }]),
					]),
			},
		);
		expect(failedWithAssistantText).toMatchObject({
			stopReason: "error",
			errorMessage: "Policy violation: cannot generate this image.",
		});

		const retryableFetch = vi.fn(
			async () =>
				new Response(JSON.stringify({ error: { message: "unavailable" } }), {
					status: 503,
					headers: { "content-type": "application/json" },
				}),
		);
		const unavailable = await generateImages(
			MODEL,
			{ input: [{ type: "text", text: "Draw" }] },
			{
				apiKey: mockToken(),
				fetch: retryableFetch,
				maxRetries: 9,
			},
		);
		expect(unavailable.stopReason).toBe("error");
		expect(retryableFetch).toHaveBeenCalledOnce();

		const oversized = await generateImages(
			MODEL,
			{ input: [{ type: "text", text: "Draw" }] },
			{
				apiKey: mockToken(),
				fetch: async () =>
					sse([
						completedResponse([
							{
								type: "image_generation_call",
								id: "ig_1",
								status: "completed",
								result: "a".repeat(24 * 1024 * 1024 + 4),
							},
						]),
					]),
			},
		);
		expect(oversized).toMatchObject({
			stopReason: "error",
			errorMessage: "ChatGPT image generation output exceeds the 24 MiB base64 limit",
		});

		const controller = new AbortController();
		const inFlightFetch = vi.fn(
			(_input: string | URL | Request, init?: RequestInit) =>
				new Promise<Response>((_resolve, reject) => {
					const signal = init?.signal;
					if (!signal) throw new Error("Expected a request signal");
					signal.addEventListener("abort", () => reject(signal.reason), { once: true });
					queueMicrotask(() => controller.abort());
				}),
		);
		const aborted = await generateImages(
			MODEL,
			{ input: [{ type: "text", text: "Draw" }] },
			{
				apiKey: mockToken(),
				fetch: inFlightFetch,
				signal: controller.signal,
			},
		);
		expect(aborted.stopReason).toBe("aborted");
		expect(inFlightFetch).toHaveBeenCalledOnce();
	});

	it("registers ChatGPT before OpenRouter in the built-in image inventory", () => {
		expect(builtinImagesProviders().map((provider) => provider.id)).toEqual(["openai-codex", "openrouter"]);
	});
});
