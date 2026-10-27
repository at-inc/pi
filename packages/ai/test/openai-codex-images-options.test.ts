import { NOOP_TELEMETRY_CONTEXT } from "@earendil-works/pi-telemetry";
import { describe, expect, it, vi } from "vitest";
import { generateImages } from "../src/api/openai-codex-images.ts";
import type { ImageModel } from "../src/types.ts";

describe("Codex image request options", () => {
	it("accepts v1.4 telemetry and forwards client identity without enabling retries or caching", async () => {
		const model: ImageModel<"openai-codex-images"> = {
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
		const identity = { userAgent: { custom: "image-client" } };
		const payload = Buffer.from(
			JSON.stringify({ "https://api.openai.com/auth": { chatgpt_account_id: "acc_test" } }),
		).toString("base64");
		const fetch = vi.fn<typeof globalThis.fetch>(async (input, init) => {
			const request = new Request(input, init);
			expect(request.url).toBe("https://chatgpt.com/backend-api/codex/images/generations");
			expect(request.headers.get("user-agent")).toBe("image-client");
			expect(await request.json()).toEqual({ model: "gpt-image-2", prompt: "Draw a circle", n: 1 });
			return Response.json({ created: 1, data: [{ b64_json: "aW1hZ2U=" }] });
		});
		const options = {
			apiKey: `header.${payload}.signature`,
			fetch,
			telemetry: NOOP_TELEMETRY_CONTEXT,
			identity,
			maxRetries: 4,
		};
		const result = await generateImages(model, { input: [{ type: "text", text: "Draw a circle" }] }, options);

		expect(result.stopReason).toBe("stop");
		expect(result.output).toEqual([{ type: "image", data: "aW1hZ2U=", mimeType: "image/png" }]);
		expect(fetch).toHaveBeenCalledOnce();
		fetch.mockClear();
		fetch.mockImplementation(async () => Response.json({ error: { message: "unavailable" } }, { status: 503 }));
		const failed = await generateImages(model, { input: [{ type: "text", text: "Draw a circle" }] }, options);
		expect(failed.stopReason).toBe("error");
		expect(fetch).toHaveBeenCalledOnce();
	});
});
