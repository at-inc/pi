// Models.login and logout of the facade over providers made by createProvider. The TS package's own login test
// (models-runtime.test.ts) registers a plain-object provider, which the models slice bridges.

import type { ApiKeyAuth, AuthInteraction, LoginOptions, OAuthAuth, ProviderStreams } from "@at-inc/pi-ai";
import { describe, expect, it } from "vitest";
import { InMemoryCredentialStore } from "../../src/auth/credential-store.ts";
import { createModels, createProvider } from "../../src/models.ts";

const streams: ProviderStreams = {
	stream: () => {
		throw new Error("not streamed");
	},
	streamSimple: () => {
		throw new Error("not streamed");
	},
};

const interaction: AuthInteraction = { prompt: async () => "unused", notify: () => {} };

function provider(auth: { apiKey?: ApiKeyAuth; oauth?: OAuthAuth }) {
	return createProvider({ id: "p1", name: "P1", auth, models: [], api: streams });
}

describe("Models.login and logout", () => {
	it("stores the credential of an api-key login and deletes it on logout", async () => {
		const credentials = new InMemoryCredentialStore();
		const apiKey: ApiKeyAuth = {
			name: "Key",
			resolve: async () => undefined,
			login: async () => ({ type: "api_key", key: "logged-in" }),
		};
		const models = createModels({ credentials });
		models.setProvider(provider({ apiKey }));

		const credential = await models.login("p1", "api_key", interaction);
		expect(credential).toEqual({ type: "api_key", key: "logged-in" });
		expect(await credentials.read("p1")).toEqual(credential);

		await models.logout("p1");
		expect(await credentials.read("p1")).toBeUndefined();
	});

	it("passes the interaction and login options to an OAuth login and stores its credential", async () => {
		const credentials = new InMemoryCredentialStore();
		const seen: { signal?: AbortSignal; options?: LoginOptions } = {};
		const oauth: OAuthAuth = {
			name: "OAuth",
			login: async (login, options) => {
				seen.signal = login.signal;
				seen.options = options;
				await login.prompt({ type: "text", message: "code?" });
				return { type: "oauth", access: "a", refresh: "r", expires: 1 };
			},
			refresh: async (credential) => credential,
			toAuth: async (credential) => ({ apiKey: credential.access }),
		};
		const models = createModels({ credentials });
		models.setProvider(provider({ oauth }));

		const credential = await models.login("p1", "oauth", interaction, {
			agentName: "test-agent",
			getDeviceId: () => "device-1",
		});
		expect(credential).toEqual({ type: "oauth", access: "a", refresh: "r", expires: 1 });
		expect(seen.signal).toBeInstanceOf(AbortSignal);
		expect(seen.options?.agentName).toBe("test-agent");
		expect(seen.options?.getDeviceId?.()).toBe("device-1");
		expect(await credentials.read("p1")).toEqual(credential);
	});

	it("rejects a login type the provider has no login for and an unknown provider", async () => {
		const models = createModels({ credentials: new InMemoryCredentialStore() });
		models.setProvider(provider({ apiKey: { name: "Key", resolve: async () => undefined } }));

		await expect(models.login("p1", "api_key", interaction)).rejects.toThrow("P1 does not support api_key login");
		await expect(models.login("p1", "oauth", interaction)).rejects.toThrow("P1 does not support oauth login");
		await expect(models.login("missing", "oauth", interaction)).rejects.toThrow("Unknown provider: missing");
	});

	it("rejects an aborted login before calling the auth", async () => {
		let called = false;
		const models = createModels({ credentials: new InMemoryCredentialStore() });
		models.setProvider(
			provider({
				apiKey: {
					name: "Key",
					resolve: async () => undefined,
					login: async () => {
						called = true;
						return { type: "api_key", key: "k" };
					},
				},
			}),
		);
		const controller = new AbortController();
		controller.abort(new Error("stopped"));

		await expect(models.login("p1", "api_key", { ...interaction, signal: controller.signal })).rejects.toThrow(
			"stopped",
		);
		expect(called).toBe(false);
	});
});
