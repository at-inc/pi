import { describe, expect, it, vi } from "vitest";
import { generateImages } from "../src/api/openai-codex-images.ts";
import { InMemoryCredentialStore } from "../src/auth/credential-store.ts";
import { generateImages as generateCompatImages, getImageModel } from "../src/compat.ts";
import { createModels } from "../src/models.ts";
import { builtinImagesProviders, builtinModels, getBuiltinImageModel } from "../src/providers/all.ts";
import { openaiCodexImagesProvider } from "../src/providers/openai-codex-images.ts";
import type { ImageModel, ImagesContext } from "../src/types.ts";

const MODEL: ImageModel<"openai-codex-images"> = {
	type: "image",
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
const CONTEXT: ImagesContext = { input: [{ type: "text", text: "Draw a fox" }] };

function mockToken(): string {
	const payload = Buffer.from(
		JSON.stringify({ "https://api.openai.com/auth": { chatgpt_account_id: "acc_test" } }),
	).toString("base64");
	return `header.${payload}.signature`;
}

describe("OpenAI Codex images", () => {
	it.each(["", "/codex", "/codex/responses", "/codex/"])(
		"generates images through the native endpoint with base URL suffix %j",
		async (suffix) => {
			const fetch = vi.fn<typeof globalThis.fetch>(async (input, init) => {
				const request = new Request(input, init);
				expect(request.url).toBe(`${MODEL.baseUrl}/codex/images/generations`);
				expect(request.method).toBe("POST");
				expect(request.headers.get("authorization")).toBe(`Bearer ${mockToken()}`);
				expect(request.headers.get("chatgpt-account-id")).toBe("acc_test");
				expect(request.headers.get("x-codex-image-turn-id")).toBeTruthy();
				expect(request.headers.get("user-agent")).toBe("image-test-client");
				expect(request.headers.get("x-test-header")).toBe("preserved");
				expect(init?.signal).toBeInstanceOf(AbortSignal);
				expect(await request.json()).toEqual({ model: "gpt-image-2", prompt: "Draw a fox", n: 1 });
				return Response.json(
					{ created: 1, data: [{ b64_json: IMAGE_DATA }] },
					{
						headers: { "x-codex-imagegen-request-id": "image-request-1" },
					},
				);
			});
			const onResponse = vi.fn();
			const result = await generateImages({ ...MODEL, baseUrl: `${MODEL.baseUrl}${suffix}` }, CONTEXT, {
				apiKey: mockToken(),
				fetch,
				onResponse,
				identity: { userAgent: { custom: "image-test-client" } },
				headers: { "x-test-header": "preserved" },
			});
			expect(result).toMatchObject({
				stopReason: "stop",
				responseId: "image-request-1",
				output: [{ type: "image", data: IMAGE_DATA, mimeType: "image/png" }],
			});
			expect(fetch).toHaveBeenCalledOnce();
			expect(onResponse).toHaveBeenCalledWith(
				expect.objectContaining({ status: 200 }),
				expect.objectContaining({ id: MODEL.id }),
			);
		},
	);

	it("edits reference images through the native JSON endpoint", async () => {
		const fetch = vi.fn<typeof globalThis.fetch>(async (input, init) => {
			const request = new Request(input, init);
			expect(request.url).toBe(`${MODEL.baseUrl}/codex/images/edits`);
			expect(await request.json()).toEqual({
				model: "gpt-image-2",
				prompt: "Add a hat",
				n: 1,
				images: [{ image_url: `data:image/png;base64,${IMAGE_DATA}` }],
			});
			return Response.json({ created: 1, data: [{ b64_json: IMAGE_DATA }] });
		});
		const result = await generateImages(
			MODEL,
			{
				input: [
					{ type: "text", text: "Add a hat" },
					{ type: "image", data: IMAGE_DATA, mimeType: "image/png" },
				],
			},
			{ apiKey: mockToken(), fetch },
		);
		expect(result).toMatchObject({
			stopReason: "stop",
			output: [{ type: "image", data: IMAGE_DATA, mimeType: "image/png" }],
		});
		expect(fetch).toHaveBeenCalledOnce();
	});

	it.each([400, 403, 429, 503])("preserves provider errors without retrying HTTP %i", async (status) => {
		const fetch = vi.fn<typeof globalThis.fetch>(async () =>
			Response.json(
				{
					error: { message: "Policy violation: cannot generate this image." },
				},
				{ status, headers: { "x-codex-imagegen-request-id": "failed-image-request" } },
			),
		);
		const onResponse = vi.fn();
		const result = await generateImages(MODEL, CONTEXT, { apiKey: mockToken(), fetch, onResponse, maxRetries: 9 });
		expect(result).toMatchObject({ stopReason: "error", responseId: "failed-image-request", output: [] });
		expect(result.errorMessage).toContain("Policy violation: cannot generate this image.");
		expect(fetch).toHaveBeenCalledOnce();
		expect(onResponse).toHaveBeenCalledWith(expect.objectContaining({ status }), MODEL);
	});

	it.each([
		{ data: [], error: "ChatGPT image generation completed without an image" },
		{ data: [{ b64_json: "not base64" }], error: "ChatGPT returned malformed image data" },
		{ data: [{ b64_json: IMAGE_DATA }, { b64_json: null }], error: "ChatGPT returned malformed image data" },
		{
			data: [{ b64_json: "a".repeat(24 * 1024 * 1024 + 4) }],
			error: "ChatGPT image generation output exceeds the 24 MiB base64 limit",
		},
	])("rejects missing, malformed, or oversized image output %#", async ({ data, error }) => {
		const result = await generateImages(MODEL, CONTEXT, {
			apiKey: mockToken(),
			fetch: async () => Response.json({ created: 1, data }),
		});
		expect(result).toMatchObject({ stopReason: "error", errorMessage: error, output: [] });
	});

	it("honors in-flight cancellation", async () => {
		const controller = new AbortController();
		const fetch = vi.fn<typeof globalThis.fetch>(
			(_input, init) =>
				new Promise<Response>((_resolve, reject) => {
					const signal = init?.signal;
					if (!signal) throw new Error("Expected a request signal");
					signal.addEventListener("abort", () => reject(signal.reason), { once: true });
					queueMicrotask(() => controller.abort());
				}),
		);
		const result = await generateImages(MODEL, CONTEXT, { apiKey: mockToken(), fetch, signal: controller.signal });
		expect(result).toMatchObject({ stopReason: "aborted", output: [] });
		expect(fetch).toHaveBeenCalledOnce();
	});

	it("does not send a cancelled request", async () => {
		const fetch = vi.fn<typeof globalThis.fetch>();
		const result = await generateImages(MODEL, CONTEXT, { apiKey: mockToken(), fetch, signal: AbortSignal.abort() });
		expect(result.stopReason).toBe("aborted");
		expect(fetch).not.toHaveBeenCalled();
	});

	it("bounds the response body with the image timeout", async () => {
		const fetch = vi.fn<typeof globalThis.fetch>(
			async (_input, init) =>
				new Response(
					new ReadableStream({
						start(controller) {
							init?.signal?.addEventListener("abort", () => controller.error(init.signal?.reason), {
								once: true,
							});
						},
					}),
					{ headers: { "content-type": "application/json" } },
				),
		);
		const result = await generateImages(MODEL, CONTEXT, { apiKey: mockToken(), fetch, timeoutMs: 25 });
		expect(result).toMatchObject({
			stopReason: "error",
			errorMessage: "ChatGPT image generation timed out after 25ms",
			output: [],
		});
		expect(fetch).toHaveBeenCalledOnce();
	});

	it("passes the native payload to hooks and rejects invalid replacements", async () => {
		const fetch = vi.fn<typeof globalThis.fetch>(async (input, init) => {
			expect(await new Request(input, init).json()).toEqual({
				model: "gpt-image-2",
				prompt: "Draw a fox",
				n: 1,
				quality: "low",
			});
			return Response.json({ created: 1, data: [{ b64_json: IMAGE_DATA }] });
		});
		const result = await generateImages(MODEL, CONTEXT, {
			apiKey: mockToken(),
			fetch,
			onPayload: (payload, model) => {
				expect(model).toEqual(MODEL);
				expect(payload).toEqual({ model: "gpt-image-2", prompt: "Draw a fox", n: 1 });
				return { ...(payload as object), quality: "low" };
			},
		});
		expect(result.stopReason).toBe("stop");
		fetch.mockClear();
		const invalid = await generateImages(MODEL, CONTEXT, { apiKey: mockToken(), fetch, onPayload: () => null });
		expect(invalid).toMatchObject({
			stopReason: "error",
			errorMessage: "Codex image generation payload hook must return an object",
		});
		expect(fetch).not.toHaveBeenCalled();
	});

	it("registers ChatGPT before OpenRouter in the built-in image inventory", () => {
		expect(builtinImagesProviders().map((provider) => provider.id)).toEqual(["openai-codex", "openrouter"]);
	});

	it("generates Codex images through the unified collection, image-only factory, and global compat API", async () => {
		const credentials = new InMemoryCredentialStore();
		await credentials.modify("openai-codex", async () => ({
			type: "oauth",
			access: mockToken(),
			refresh: "mock-refresh-token",
			expires: Date.now() + 3_600_000,
		}));
		const models = builtinModels({ credentials });
		const model = models.getModelOfType("image", "openai-codex", "chatgpt-image-generation")!;
		expect(model).toMatchObject(MODEL);
		expect(getBuiltinImageModel("openai-codex", "chatgpt-image-generation")).toEqual(model);
		expect(getImageModel("openai-codex", "chatgpt-image-generation")).toEqual(model);
		const imageModels = createModels({ credentials });
		imageModels.setProvider(openaiCodexImagesProvider());
		expect(imageModels.getModels()).toEqual([]);
		expect(imageModels.getModelsOfType("image")).toEqual([model]);
		const options = { fetch: async () => Response.json({ created: 1, data: [{ b64_json: IMAGE_DATA }] }) };
		for (const result of [
			await models.generateImages(model, CONTEXT, options),
			await imageModels.generateImages(model, CONTEXT, options),
			await generateCompatImages(model, CONTEXT, { ...options, apiKey: mockToken() }),
		]) {
			expect(result).toMatchObject({
				stopReason: "stop",
				output: [{ type: "image", data: IMAGE_DATA, mimeType: "image/png" }],
			});
		}
	});
});
