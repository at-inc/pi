import Anthropic from "@anthropic-ai/sdk";
import type { MessageCreateParamsStreaming } from "@anthropic-ai/sdk/resources/beta/messages/messages.js";
import { Type } from "typebox";
import { afterEach, describe, expect, it, vi } from "vitest";
import { type AnthropicOptions, stream, streamSimple } from "../src/api/anthropic-messages.ts";
import type { Model } from "../src/types.ts";
import { formatUserAgent } from "../src/utils/identity.ts";
import { normalizeContext, type TranscriptContext } from "../src/utils/transcript.ts";

interface RecordedRequest {
	request: Request;
	body: MessageCreateParamsStreaming & { temperature?: number };
}

const model: Model<"anthropic-messages"> = {
	id: "claude-sonnet-4-6",
	name: "Claude Sonnet 4.6",
	api: "anthropic-messages",
	provider: "anthropic",
	baseUrl: "https://anthropic.test",
	reasoning: true,
	input: ["text"],
	cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0 },
	contextWindow: 200000,
	maxTokens: 4096,
};

const context = normalizeContext({
	systemPrompt: "System prompt.",
	messages: [{ role: "user", content: "Hello", timestamp: 1 }],
});

function createFetch(requests: RecordedRequest[]): typeof globalThis.fetch {
	return async (input, init) => {
		const request = new Request(input, init);
		expect(new URL(request.url).pathname).toBe("/v1/messages");
		requests.push({ request, body: JSON.parse(await request.text()) });
		const events = [
			{
				type: "message_start",
				message: { id: "msg_test", model: model.id, usage: { input_tokens: 1, output_tokens: 0 } },
			},
			{ type: "message_delta", delta: { stop_reason: "end_turn" }, usage: { output_tokens: 1 } },
			{ type: "message_stop" },
		];
		return new Response(events.map((event) => `event: ${event.type}\ndata: ${JSON.stringify(event)}\n\n`).join(""), {
			status: 200,
			headers: { "content-type": "text/event-stream" },
		});
	};
}

async function captureRequest(
	target = model,
	options: AnthropicOptions = {},
	transcript: TranscriptContext = context,
): Promise<RecordedRequest> {
	const requests: RecordedRequest[] = [];
	const message = await stream(target, transcript, {
		apiKey: "test-key",
		...options,
		fetch: createFetch(requests),
	}).result();
	expect(message.stopReason, message.errorMessage).toBe("stop");
	expect(requests).toHaveLength(1);
	return requests[0];
}

afterEach(() => {
	vi.unstubAllEnvs();
});

describe("Anthropic SDK v1 wire compatibility", () => {
	it.each([0, 1])("uses the native version and Bearer auth without temperature=%s", async (temperature) => {
		const { request, body } = await captureRequest(model, { temperature });

		expect(request.headers.get("anthropic-version")).toBe("2026-10-21");
		expect(request.headers.get("authorization")).toBe("Bearer test-key");
		expect(request.headers.has("x-api-key")).toBe(false);
		expect(request.headers.get("user-agent")).toBe(formatUserAgent());
		expect(body).not.toHaveProperty("temperature");
		expect(body).not.toHaveProperty("top_p");
		expect(body).not.toHaveProperty("top_k");
	});

	it.each(["kimi-coding", "minimax", "xiaomi", "openrouter", "vendor-proxy"])(
		"preserves the legacy version, X-Api-Key, and supported temperature for %s",
		async (provider) => {
			const { request, body } = await captureRequest({ ...model, provider }, { temperature: 0 });

			expect(request.headers.get("anthropic-version")).toBe("2023-06-01");
			expect(request.headers.get("x-api-key")).toBe("test-key");
			expect(request.headers.has("authorization")).toBe(false);
			expect(body.temperature).toBe(0);
		},
	);

	it("keeps Copilot Bearer auth on the compatible API version", async () => {
		const { request, body } = await captureRequest({ ...model, provider: "github-copilot" }, { temperature: 0 });

		expect(request.headers.get("anthropic-version")).toBe("2023-06-01");
		expect(request.headers.get("authorization")).toBe("Bearer test-key");
		expect(request.headers.has("x-api-key")).toBe(false);
		expect(request.headers.get("x-initiator")).toBe("user");
		expect(body.temperature).toBe(0);
	});

	it("uses the latest Claude Code OAuth user agent instead of the configured identity", async () => {
		const { request, body } = await captureRequest(model, {
			apiKey: "sk-ant-oat-test",
			temperature: 0,
			identity: { userAgent: { custom: "not-claude-code" } },
		});

		expect(request.headers.get("user-agent")).toBe("claude-cli/2.1.307");
		expect(request.headers.get("authorization")).toBe("Bearer sk-ant-oat-test");
		expect(request.headers.has("x-api-key")).toBe(false);
		expect(request.headers.get("anthropic-version")).toBe("2026-10-21");
		expect(request.headers.get("anthropic-beta")).toContain("oauth-2025-04-20");
		expect(body.system).toEqual([
			expect.objectContaining({ text: "You are Claude Code, Anthropic's official CLI for Claude." }),
			expect.objectContaining({ text: "System prompt." }),
		]);
		expect(body).not.toHaveProperty("temperature");
	});

	it("lets mixed-case caller headers override model version, auth, and identity", async () => {
		const { request, body } = await captureRequest(
			{
				...model,
				headers: {
					"Anthropic-Version": "2026-10-21",
					Authorization: "Bearer model-key",
					"User-Agent": "model-client",
				},
			},
			{
				temperature: 0,
				headers: {
					"ANTHROPIC-version": "2023-06-01",
					authorization: "Bearer caller-key",
					"user-agent": "caller-client",
				},
			},
		);

		expect(request.headers.get("anthropic-version")).toBe("2023-06-01");
		expect(request.headers.get("authorization")).toBe("Bearer caller-key");
		expect(request.headers.get("user-agent")).toBe("caller-client");
		expect(body.temperature).toBe(0);
	});

	it("omits sampling when a compatible provider explicitly selects the native version", async () => {
		const { request, body } = await captureRequest(
			{ ...model, provider: "vendor-proxy", headers: { "Anthropic-Version": "2026-10-21" } },
			{ temperature: 0 },
		);

		expect(request.headers.get("anthropic-version")).toBe("2026-10-21");
		expect(request.headers.get("x-api-key")).toBe("test-key");
		expect(request.headers.has("authorization")).toBe(false);
		expect(body).not.toHaveProperty("temperature");
	});

	it("honors explicit API-version suppression without generating legacy sampling", async () => {
		const { request, body } = await captureRequest(
			{ ...model, provider: "vendor-proxy", headers: { "Anthropic-Version": "2023-06-01" } },
			{ temperature: 0, headers: { "anthropic-version": null } },
		);

		expect(request.headers.has("anthropic-version")).toBe(false);
		expect(body).not.toHaveProperty("temperature");
	});

	it("honors Authorization null suppression after a model-owned Bearer header", async () => {
		const { request } = await captureRequest(
			{ ...model, headers: { Authorization: "Bearer model-key" } },
			{ headers: { authorization: null, "X-Api-Key": "gateway-key" } },
		);

		expect(request.headers.has("authorization")).toBe(false);
		expect(request.headers.get("x-api-key")).toBe("gateway-key");
	});

	it.each(["anthropic", "vendor-proxy"])("allows explicit Bearer auth on %s", async (provider) => {
		const { request } = await captureRequest(
			{ ...model, provider },
			{ apiKey: undefined, headers: { Authorization: "Bearer gateway-token" } },
		);

		expect(request.headers.get("authorization")).toBe("Bearer gateway-token");
		expect(request.headers.has("x-api-key")).toBe(false);
	});

	it("does not resolve ambient SDK credentials for model-owned auth", async () => {
		vi.stubEnv("ANTHROPIC_API_KEY", "ambient-key");
		vi.stubEnv("ANTHROPIC_AUTH_TOKEN", "ambient-token");
		vi.stubEnv("ANTHROPIC_PROFILE", "missing-profile");
		vi.stubEnv("ANTHROPIC_FEDERATION_RULE_ID", "fdrl_ambient");
		vi.stubEnv("ANTHROPIC_ORGANIZATION_ID", "org_ambient");
		vi.stubEnv("ANTHROPIC_IDENTITY_TOKEN_FILE", "/missing-identity.jwt");
		const { request } = await captureRequest(
			{ ...model, headers: { "X-Api-Key": "model-key" } },
			{ apiKey: undefined, headers: { Authorization: null } },
		);

		expect(request.headers.get("x-api-key")).toBe("model-key");
		expect(request.headers.has("authorization")).toBe(false);
	});

	it("accepts model-owned auth through streamSimple", async () => {
		const requests: RecordedRequest[] = [];
		const message = await streamSimple({ ...model, headers: { Authorization: "Bearer model-key" } }, context, {
			fetch: createFetch(requests),
			env: {},
		}).result();

		expect(message.stopReason, message.errorMessage).toBe("stop");
		expect(requests).toHaveLength(1);
		expect(requests[0].request.headers.get("authorization")).toBe("Bearer model-key");
	});

	it.each([undefined, "2023-06-01"])(
		"preserves injected client defaults with explicit version %s",
		async (version) => {
			const requests: RecordedRequest[] = [];
			const client = new Anthropic({ apiKey: "injected-key", baseURL: model.baseUrl, fetch: createFetch(requests) });
			const message = await stream({ ...model, provider: "google-vertex" }, context, {
				client,
				temperature: 0,
				...(version === undefined ? {} : { headers: { "Anthropic-Version": version } }),
			}).result();

			expect(message.stopReason, message.errorMessage).toBe("stop");
			expect(requests).toHaveLength(1);
			expect(requests[0].request.headers.get("authorization")).toBe("Bearer injected-key");
			expect(requests[0].request.headers.get("anthropic-version")).toBe(version ?? "2026-10-21");
			if (version === undefined) expect(requests[0].body).not.toHaveProperty("temperature");
			else expect(requests[0].body.temperature).toBe(0);
		},
	);

	it("serializes native system messages and tool changes with role-specific SDK types", async () => {
		const initialTool = { name: "initial", description: "Initial tool", parameters: Type.Object({}) };
		const addedTool = { name: "added", description: "Added tool", parameters: Type.Object({}) };
		const { request, body } = await captureRequest(
			{ ...model, compat: { supportsMidConvoSystemMessages: true, supportsMidConvoToolChanges: true } },
			{ temperature: 0 },
			normalizeContext({
				messages: [
					{ role: "system", content: "Initial prompt", toolsAdded: [initialTool], timestamp: 0 },
					{ role: "user", content: "Hello", timestamp: 1 },
					{
						role: "system",
						content: "Updated guidance",
						toolsRemoved: [{ name: "initial" }],
						toolsAdded: [addedTool],
						timestamp: 2,
					},
				],
			}),
		);

		expect(request.headers.get("anthropic-version")).toBe("2026-10-21");
		expect(request.headers.get("anthropic-beta")).toContain("inline-tools-2026-09-15");
		expect(body.tools).toMatchObject([{ name: "initial" }, { defer_loading: true }]);
		expect(body.messages.at(-1)).toMatchObject({
			role: "system",
			content: [
				{ type: "text", text: "Updated guidance" },
				{ type: "tool_removal", tool: { type: "tool_reference", name: "initial" } },
				{
					type: "tool_addition",
					tool: { type: "tool_definition", definition: { name: "added" } },
					cache_control: { type: "ephemeral" },
				},
			],
		});
		expect(body).not.toHaveProperty("temperature");
	});
});
