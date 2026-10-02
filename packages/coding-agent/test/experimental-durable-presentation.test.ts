import type { EntryRecord } from "@earendil-works/pi-durable";
import { describe, expect, test } from "vitest";
import { createLoginProviderOptions } from "../src/experimental/durable/login.ts";
import { lastCacheHitRate } from "../src/experimental/durable/usage.ts";

describe("durable presentation helpers", () => {
	test("offers interactive and ambient provider login methods with auth status", () => {
		const providers = createLoginProviderOptions(
			[
				{
					id: "openai",
					name: "OpenAI",
					oauth: { name: "ChatGPT", subscription: true },
					apiKey: { name: "API key", interactive: true },
				},
				{ id: "bedrock", name: "Amazon Bedrock", apiKey: { name: "AWS credentials", interactive: false } },
			],
			(providerId) =>
				providerId === "openai" ? { configured: true, label: "stored credential" } : { configured: false },
			(providerId) => providerId === "openai",
		);

		expect(providers).toEqual([
			{
				id: "bedrock",
				name: "Amazon Bedrock",
				authType: "api_key",
				interactive: false,
				methodName: "AWS credentials",
			},
			{
				id: "openai",
				name: "OpenAI",
				authType: "oauth",
				status: { type: "oauth", source: "stored credential" },
				interactive: true,
				methodName: "ChatGPT",
				subscription: true,
			},
			{
				id: "openai",
				name: "OpenAI",
				authType: "api_key",
				status: { type: "oauth", source: "stored credential" },
				interactive: true,
				methodName: "API key",
			},
		]);
	});

	test("uses the newest successful assistant response for cache-hit rate", () => {
		const entry = (id: number, stopReason: string, input: number, cacheRead: number, cacheWrite: number) =>
			({
				id,
				kind: "pi.assistant",
				model: [
					{
						role: "assistant",
						stopReason,
						usage: { input, output: 1, cacheRead, cacheWrite },
					},
				],
			}) as unknown as EntryRecord;

		expect(lastCacheHitRate([entry(1, "stop", 100, 300, 100), entry(2, "error", 500, 0, 0)])).toBe(60);
		expect(lastCacheHitRate([entry(1, "stop", 100, 300, 100), entry(2, "stop", 0, 0, 0)])).toBeUndefined();
		expect(lastCacheHitRate([])).toBeUndefined();
	});
});
