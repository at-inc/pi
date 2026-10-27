import { beforeEach, describe, expect, it, vi } from "vitest";
import { fauxAssistantMessage } from "../src/providers/faux.ts";
import { opencodeGoProvider } from "../src/providers/opencode-go.ts";
import type { ProviderStreams } from "../src/types.ts";
import { AssistantMessageEventStream } from "../src/utils/event-stream.ts";
import { normalizeContext } from "../src/utils/transcript.ts";

const googleStreams = vi.hoisted(() => ({
	stream: vi.fn<ProviderStreams["stream"]>(),
	streamSimple: vi.fn<ProviderStreams["streamSimple"]>(),
}));

vi.mock("../src/api/google-generative-ai.lazy.ts", () => ({
	googleGenerativeAIApi: () => googleStreams,
}));

beforeEach(() => {
	vi.clearAllMocks();
	for (const method of ["stream", "streamSimple"] as const) {
		googleStreams[method].mockImplementation(() => {
			const stream = new AssistantMessageEventStream();
			const message = fauxAssistantMessage("ok");
			stream.push({ type: "done", reason: "stop", message });
			stream.end(message);
			return stream;
		});
	}
});

describe("OpenCode Go Gemini dispatch", () => {
	it.each(["stream", "streamSimple"] as const)(
		"routes %s through Google Generative AI with the OpenCode session header",
		async (method) => {
			const provider = opencodeGoProvider();
			const model = provider.getModels().find((model) => model.id === "gemini-4-argon");
			expect(model).toMatchObject({
				api: "google-generative-ai",
				provider: "opencode-go",
				baseUrl: "https://opencode.ai/inference/go/v1",
			});
			if (!model) throw new Error("OpenCode Go Gemini model is missing");
			const context = normalizeContext({ messages: [{ role: "user", content: "hello", timestamp: 0 }] });
			const result = await provider[method](model, context, {
				apiKey: "test-key",
				sessionId: "conversation-1",
				headers: { "x-custom": "value" },
			}).result();

			expect(result.stopReason).toBe("stop");
			expect(googleStreams[method]).toHaveBeenCalledExactlyOnceWith(model, context, {
				apiKey: "test-key",
				sessionId: "conversation-1",
				headers: { "x-custom": "value", "x-opencode-session": "conversation-1" },
			});
		},
	);
});
