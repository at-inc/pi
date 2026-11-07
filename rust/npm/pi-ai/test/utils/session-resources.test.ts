// session-resources and loadRadiusGatewayConfig of the facade, which the TS package's own tests do not cover.

import { afterEach, describe, expect, it, vi } from "vitest";
import { cleanupSessionResources, registerSessionResourceCleanup } from "../../src/index.ts";
import { loadRadiusGatewayConfig } from "../../src/providers/radius-config.ts";

describe("session resources", () => {
	it("runs registered cleanups with the session id until they are unregistered", () => {
		const seen: (string | undefined)[] = [];
		const cleanup = (sessionId?: string) => {
			seen.push(sessionId);
		};
		const unregister = registerSessionResourceCleanup(cleanup);
		const again = registerSessionResourceCleanup(cleanup);
		cleanupSessionResources("session-1");
		cleanupSessionResources();
		expect(seen).toEqual(["session-1", undefined]);
		again();
		unregister();
		cleanupSessionResources("session-2");
		expect(seen).toEqual(["session-1", undefined]);
	});

	it("runs every cleanup and throws an AggregateError of the failures", () => {
		const ran: string[] = [];
		const unregisterFailing = registerSessionResourceCleanup(() => {
			throw new Error("socket stuck");
		});
		const unregisterOk = registerSessionResourceCleanup(() => {
			ran.push("ok");
		});
		let thrown: unknown;
		try {
			cleanupSessionResources("s");
		} catch (error) {
			thrown = error;
		}
		unregisterFailing();
		unregisterOk();
		expect(ran).toEqual(["ok"]);
		expect(thrown).toBeInstanceOf(AggregateError);
		expect((thrown as AggregateError).message).toBe("Failed to cleanup session resources");
		expect((thrown as AggregateError).errors).toMatchObject([{ message: "socket stuck" }]);
	});
});

describe("loadRadiusGatewayConfig", () => {
	afterEach(() => {
		vi.unstubAllGlobals();
	});

	it("reads the gateway's /v1/config with the API key and rejects a failed response", async () => {
		const requests: { url: string; authorization: string | null }[] = [];
		let status = 200;
		vi.stubGlobal("fetch", async (url: string | URL, init?: RequestInit) => {
			requests.push({ url: String(url), authorization: new Headers(init?.headers).get("authorization") });
			return new Response(status === 200 ? JSON.stringify({ models: [] }) : "  down  ", { status });
		});
		await loadRadiusGatewayConfig("https://radius.example", "key-1").catch(() => undefined);
		expect(requests[0]).toEqual({ url: "https://radius.example/v1/config", authorization: "Bearer key-1" });
		status = 503;
		await expect(loadRadiusGatewayConfig("https://radius.example")).rejects.toThrow(
			"Could not load Radius config from https://radius.example: 503: down",
		);
	});
});
