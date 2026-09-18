import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { InMemoryCredentialStore } from "../src/auth/credential-store.ts";
import { kimiCodingOAuth } from "../src/auth/oauth/kimi-coding.ts";
import type { ProviderAuthInteraction } from "../src/auth/types.ts";
import { createModels } from "../src/models.ts";
import { kimiCodingProvider } from "../src/providers/kimi-coding.ts";

const CLIENT_ID = "17e5f671-d194-4dfb-9706-5516cb48c098";
const OAUTH_HOST = "https://auth.kimi.com";

function jsonResponse(body: unknown, status: number = 200): Response {
	return new Response(JSON.stringify(body), {
		status,
		headers: { "Content-Type": "application/json" },
	});
}

function getUrl(input: unknown): string {
	if (typeof input === "string") return input;
	if (input instanceof URL) return input.toString();
	if (input instanceof Request) return input.url;
	throw new Error(`Unsupported fetch input: ${String(input)}`);
}

function deviceAuthorizationResponse(overrides?: Record<string, unknown>): Response {
	return jsonResponse({
		user_code: "ABCD-1234",
		device_code: "device-code-123",
		verification_uri: "https://www.kimi.com/code",
		verification_uri_complete: "https://www.kimi.com/code?user_code=ABCD-1234",
		interval: 5,
		expires_in: 600,
		...overrides,
	});
}

function createInteraction(events: Array<Record<string, unknown>>, region = "mainland-cn"): ProviderAuthInteraction {
	return {
		signal: new AbortController().signal,
		prompt: async (prompt) => {
			expect(prompt.type).toBe("select");
			return region;
		},
		notify: (event) => events.push(event),
	};
}

describe("Kimi Code OAuth", () => {
	beforeEach(() => {
		vi.stubEnv("KIMI_CODE_OAUTH_HOST", "");
		vi.stubEnv("KIMI_OAUTH_HOST", "");
		vi.stubEnv("KIMI_CODE_BASE_URL", "");
	});

	afterEach(() => {
		vi.restoreAllMocks();
		vi.unstubAllGlobals();
		vi.unstubAllEnvs();
		vi.useRealTimers();
	});

	it("logs in with the device authorization flow", async () => {
		vi.useFakeTimers();
		const startTime = new Date("2026-07-20T00:00:00Z");
		vi.setSystemTime(startTime);

		const events: Array<Record<string, unknown>> = [];
		const pollResponses = [
			jsonResponse({ error: "authorization_pending" }, 400),
			jsonResponse({ access_token: "access-token", refresh_token: "refresh-token", expires_in: 3600 }),
		];
		const pollTimes: number[] = [];

		vi.stubGlobal(
			"fetch",
			vi.fn(async (input: unknown, init?: RequestInit): Promise<Response> => {
				const url = getUrl(input);
				if (url === `${OAUTH_HOST}/api/oauth/device_authorization`) {
					expect(init?.method).toBe("POST");
					expect(init?.headers).toMatchObject({
						"Content-Type": "application/x-www-form-urlencoded",
						Accept: "application/json",
					});
					expect(new URLSearchParams(String(init?.body)).get("client_id")).toBe(CLIENT_ID);
					return deviceAuthorizationResponse();
				}
				if (url === `${OAUTH_HOST}/api/oauth/token`) {
					pollTimes.push(Date.now());
					const params = new URLSearchParams(String(init?.body));
					expect(params.get("grant_type")).toBe("urn:ietf:params:oauth:grant-type:device_code");
					expect(params.get("client_id")).toBe(CLIENT_ID);
					expect(params.get("device_code")).toBe("device-code-123");
					const response = pollResponses.shift();
					if (!response) throw new Error("Unexpected extra token poll");
					return response;
				}
				throw new Error(`Unexpected fetch URL: ${url}`);
			}),
		);

		const credentialPromise = kimiCodingOAuth.login(createInteraction(events));
		for (let i = 0; i < 5 && events.length === 0; i++) {
			await vi.advanceTimersByTimeAsync(0);
		}
		expect(events).toEqual([
			{
				type: "device_code",
				userCode: "ABCD-1234",
				verificationUri: "https://www.kimi.com/code?user_code=ABCD-1234",
				intervalSeconds: 5,
				expiresInSeconds: 600,
			},
		]);

		// waitBeforeFirstPoll: first poll happens after the 5s interval.
		await vi.advanceTimersByTimeAsync(4999);
		expect(pollTimes).toEqual([]);
		await vi.advanceTimersByTimeAsync(1);
		expect(pollTimes).toEqual([startTime.getTime() + 5000]);

		await vi.advanceTimersByTimeAsync(5000);
		await expect(credentialPromise).resolves.toEqual({
			type: "oauth",
			oauthHost: OAUTH_HOST,
			baseUrl: "https://api.kimi.com/coding",
			access: "access-token",
			refresh: "refresh-token",
			expires: startTime.getTime() + 10000 + 3600 * 1000,
		});
		expect(pollTimes).toEqual([startTime.getTime() + 5000, startTime.getTime() + 10000]);
	});

	it("fails when the device code expires", async () => {
		vi.useFakeTimers();
		vi.stubGlobal(
			"fetch",
			vi.fn(async (input: unknown): Promise<Response> => {
				const url = getUrl(input);
				if (url === `${OAUTH_HOST}/api/oauth/device_authorization`) {
					return deviceAuthorizationResponse();
				}
				if (url === `${OAUTH_HOST}/api/oauth/token`) {
					return jsonResponse({ error: "expired_token" }, 400);
				}
				throw new Error(`Unexpected fetch URL: ${url}`);
			}),
		);

		const credentialPromise = kimiCodingOAuth.login(createInteraction([]));
		const assertion = expect(credentialPromise).rejects.toThrow("expired");
		await vi.advanceTimersByTimeAsync(5000);
		await assertion;
	});

	it("fails when the user denies the login", async () => {
		vi.useFakeTimers();
		vi.stubGlobal(
			"fetch",
			vi.fn(async (input: unknown): Promise<Response> => {
				const url = getUrl(input);
				if (url === `${OAUTH_HOST}/api/oauth/device_authorization`) {
					return deviceAuthorizationResponse();
				}
				if (url === `${OAUTH_HOST}/api/oauth/token`) {
					return jsonResponse({ error: "access_denied" }, 400);
				}
				throw new Error(`Unexpected fetch URL: ${url}`);
			}),
		);

		const credentialPromise = kimiCodingOAuth.login(createInteraction([]));
		const assertion = expect(credentialPromise).rejects.toThrow("denied");
		await vi.advanceTimersByTimeAsync(5000);
		await assertion;
	});

	it("honors the KIMI_CODE_OAUTH_HOST override", async () => {
		vi.useFakeTimers();
		vi.stubEnv("KIMI_CODE_OAUTH_HOST", "https://auth.example.com/");

		const urls: string[] = [];
		vi.stubGlobal(
			"fetch",
			vi.fn(async (input: unknown): Promise<Response> => {
				const url = getUrl(input);
				urls.push(url);
				if (url === "https://auth.example.com/api/oauth/device_authorization") {
					return deviceAuthorizationResponse({ interval: 1 });
				}
				if (url === "https://auth.example.com/api/oauth/token") {
					return jsonResponse({ access_token: "a", refresh_token: "r", expires_in: 60 });
				}
				throw new Error(`Unexpected fetch URL: ${url}`);
			}),
		);

		const credentialPromise = kimiCodingOAuth.login(createInteraction([]));
		await vi.advanceTimersByTimeAsync(1000);
		await expect(credentialPromise).resolves.toMatchObject({ access: "a", refresh: "r" });
		expect(urls).toEqual([
			"https://auth.example.com/api/oauth/device_authorization",
			"https://auth.example.com/api/oauth/token",
		]);
	});

	it("refreshes tokens and returns a Bearer header for requests", async () => {
		vi.stubGlobal(
			"fetch",
			vi.fn(async (input: unknown, init?: RequestInit): Promise<Response> => {
				const url = getUrl(input);
				expect(url).toBe(`${OAUTH_HOST}/api/oauth/token`);
				const params = new URLSearchParams(String(init?.body));
				expect(params.get("grant_type")).toBe("refresh_token");
				expect(params.get("refresh_token")).toBe("old-refresh");
				expect(params.get("client_id")).toBe(CLIENT_ID);
				return jsonResponse({ access_token: "new-access", refresh_token: "new-refresh", expires_in: 3600 });
			}),
		);

		const before = Date.now();
		const credential = await kimiCodingOAuth.refresh(
			{
				type: "oauth",
				access: "old-access",
				refresh: "old-refresh",
				expires: before,
			},
			new AbortController().signal,
		);
		expect(credential).toEqual({
			type: "oauth",
			oauthHost: OAUTH_HOST,
			baseUrl: "https://api.kimi.com/coding",
			access: "new-access",
			refresh: "new-refresh",
			expires: expect.any(Number),
		});
		expect(credential.expires).toBeGreaterThanOrEqual(before + 3600 * 1000);

		await expect(kimiCodingOAuth.toAuth(credential)).resolves.toEqual({
			headers: { Authorization: "Bearer new-access" },
			baseUrl: "https://api.kimi.com/coding",
		});
	});

	// Same regional-login failure reported in can1357/oh-my-pi#12024.
	it("keeps international login, refresh, and model requests on kimi.ai after environment changes", async () => {
		vi.useFakeTimers();
		const urls: string[] = [];
		const events: Array<Record<string, unknown>> = [];
		vi.stubGlobal(
			"fetch",
			vi.fn(async (input: unknown) => {
				const url = getUrl(input);
				urls.push(url);
				if (url === "https://auth.kimi.ai/api/oauth/device_authorization") {
					return deviceAuthorizationResponse({
						interval: 1,
						verification_uri: "https://www.kimi.ai/code/authorize_device",
						verification_uri_complete: "https://www.kimi.ai/code/authorize_device?user_code=ABCD-1234",
					});
				}
				if (url === "https://auth.kimi.ai/api/oauth/token") {
					return jsonResponse({
						access_token: "global-access",
						refresh_token: "global-refresh",
						expires_in: 3600,
					});
				}
				throw new Error(`Unexpected fetch URL: ${url}`);
			}),
		);

		const login = kimiCodingOAuth.login(createInteraction(events, "global"));
		await vi.advanceTimersByTimeAsync(1000);
		const credential = await login;
		expect(events[0]).toMatchObject({
			verificationUri: "https://www.kimi.ai/code/authorize_device?user_code=ABCD-1234",
		});
		expect(credential).toMatchObject({ oauthHost: "https://auth.kimi.ai", baseUrl: "https://api.kimi.ai/coding" });
		vi.stubEnv("KIMI_CODE_OAUTH_HOST", OAUTH_HOST);
		vi.stubEnv("KIMI_CODE_BASE_URL", "https://api.kimi.com/coding");
		// Round-trip through storage as a restarted client would.
		const refreshed = await kimiCodingOAuth.refresh(
			JSON.parse(JSON.stringify(credential)),
			new AbortController().signal,
		);
		expect(refreshed).toMatchObject({ oauthHost: "https://auth.kimi.ai", baseUrl: "https://api.kimi.ai/coding" });
		expect(urls).toEqual([
			"https://auth.kimi.ai/api/oauth/device_authorization",
			"https://auth.kimi.ai/api/oauth/token",
			"https://auth.kimi.ai/api/oauth/token",
		]);

		vi.useRealTimers();
		const credentials = new InMemoryCredentialStore();
		await credentials.modify("kimi-coding", async () => refreshed);
		const models = createModels({ credentials });
		models.setProvider(kimiCodingProvider());
		const request = vi.fn(async (input: string | URL | Request, init?: RequestInit) => {
			expect(getUrl(input)).toBe("https://api.kimi.ai/coding/v1/messages?beta=true");
			expect(new Headers(init?.headers).get("Authorization")).toBe("Bearer global-access");
			return new Response(
				[
					'event: message_start\ndata: {"type":"message_start","message":{"id":"msg_test","usage":{"input_tokens":1,"output_tokens":0}}}',
					'event: message_delta\ndata: {"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":1}}',
					'event: message_stop\ndata: {"type":"message_stop"}',
					"",
				].join("\n\n"),
				{ headers: { "content-type": "text/event-stream" } },
			);
		});
		const result = await models.completeSimple(
			models.getModel("kimi-coding", "kimi-for-coding")!,
			{ messages: [{ role: "user", content: "Hello", timestamp: Date.now() }] },
			{ fetch: request },
		);
		expect(result.errorMessage).toBeUndefined();
		expect(result.stopReason).toBe("stop");
		expect(request).toHaveBeenCalledOnce();
	});

	it.each([
		["KIMI_CODE_OAUTH_HOST", "https://auth.kimi.ai/"],
		["KIMI_OAUTH_HOST", "https://auth.kimi.ai"],
		["KIMI_CODE_BASE_URL", "https://api.kimi.ai/coding/v1/"],
	])("infers both international endpoints from %s without a prompt", async (name, value) => {
		vi.useFakeTimers();
		vi.stubEnv(name, value);
		vi.stubGlobal(
			"fetch",
			vi.fn(async (input: unknown) => {
				const url = getUrl(input);
				if (url === "https://auth.kimi.ai/api/oauth/device_authorization")
					return deviceAuthorizationResponse({ interval: 1 });
				if (url === "https://auth.kimi.ai/api/oauth/token")
					return jsonResponse({ access_token: "a", refresh_token: "r", expires_in: 60 });
				throw new Error(`Unexpected fetch URL: ${url}`);
			}),
		);
		const interaction = createInteraction([]);
		interaction.prompt = vi.fn().mockRejectedValue(new Error("Should not prompt"));
		const login = kimiCodingOAuth.login(interaction);
		await vi.advanceTimersByTimeAsync(1000);
		await expect(login).resolves.toMatchObject({
			oauthHost: "https://auth.kimi.ai",
			baseUrl: "https://api.kimi.ai/coding",
		});
		expect(interaction.prompt).not.toHaveBeenCalled();
	});

	it("does not start device authorization when region selection is canceled", async () => {
		const fetch = vi.fn();
		vi.stubGlobal("fetch", fetch);
		const interaction = createInteraction([]);
		interaction.prompt = vi.fn().mockRejectedValue(new Error("Canceled"));
		await expect(kimiCodingOAuth.login(interaction)).rejects.toThrow("Canceled");
		expect(fetch).not.toHaveBeenCalled();
	});

	it("retries refresh on 429 and fails unauthorized on invalid_grant", async () => {
		vi.useFakeTimers();

		// 429 once, then success.
		let calls = 0;
		vi.stubGlobal(
			"fetch",
			vi.fn(async (): Promise<Response> => {
				calls += 1;
				if (calls === 1) return jsonResponse({ error: "temporarily_unavailable" }, 429);
				return jsonResponse({ access_token: "a", refresh_token: "r", expires_in: 60 });
			}),
		);

		const refreshPromise = kimiCodingOAuth.refresh(
			{
				type: "oauth",
				access: "old",
				refresh: "old",
				expires: 0,
			},
			new AbortController().signal,
		);
		await vi.advanceTimersByTimeAsync(1000);
		await expect(refreshPromise).resolves.toMatchObject({ access: "a" });
		expect(calls).toBe(2);

		// invalid_grant is not retried.
		vi.stubGlobal(
			"fetch",
			vi.fn(async (): Promise<Response> => jsonResponse({ error: "invalid_grant" }, 400)),
		);
		await expect(
			kimiCodingOAuth.refresh(
				{ type: "oauth", access: "old", refresh: "old", expires: 0 },
				new AbortController().signal,
			),
		).rejects.toThrow("unauthorized");
	});
});
