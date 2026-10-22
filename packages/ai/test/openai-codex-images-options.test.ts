import { NOOP_TELEMETRY_CONTEXT } from "@earendil-works/pi-telemetry";
import { describe, expect, it, vi } from "vitest";
import { generateImages } from "../src/api/openai-codex-images.ts";
import type { OpenAICodexResponsesOptions } from "../src/api/openai-codex-responses.ts";
import type { AssistantMessage, ImageModel } from "../src/types.ts";

const captured = vi.hoisted(() => ({ options: undefined as OpenAICodexResponsesOptions | undefined }));

vi.mock("../src/api/openai-codex-responses.ts", () => ({
	stream: (_model: unknown, _context: unknown, options: OpenAICodexResponsesOptions) => {
		captured.options = options;
		return {
			result: async (): Promise<AssistantMessage> => {
				options.onImageGenerationPartialImage?.(0, "aW1hZ2U=");
				return {
					role: "assistant",
					api: "openai-codex-responses",
					provider: "openai-codex",
					model: "gpt-6-luna",
					content: [],
					usage: {
						input: 0,
						output: 0,
						cacheRead: 0,
						cacheWrite: 0,
						totalTokens: 0,
						cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0, total: 0 },
					},
					stopReason: "stop",
					timestamp: 0,
				};
			},
		};
	},
}));

describe("Codex image request options", () => {
	it("forwards the v1.4 parent span and client identity without enabling retries or caching", async () => {
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
		const result = await generateImages(
			model,
			{ input: [{ type: "text", text: "Draw a circle" }] },
			{
				telemetry: NOOP_TELEMETRY_CONTEXT,
				identity,
				maxRetries: 4,
			},
		);

		expect(result.stopReason).toBe("stop");
		expect(result.output).toEqual([{ type: "image", data: "aW1hZ2U=", mimeType: "image/png" }]);
		expect(captured.options?.telemetry).toBe(NOOP_TELEMETRY_CONTEXT);
		expect(captured.options?.identity).toBe(identity);
		expect(captured.options?.maxRetries).toBe(0);
		expect(captured.options?.cacheRetention).toBe("none");
		expect(captured.options).not.toHaveProperty("telemetryContext");
	});
});
