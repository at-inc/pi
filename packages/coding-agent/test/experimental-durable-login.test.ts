import { mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fauxProvider } from "@at-inc/pi-ai";
import { afterEach, expect, test, vi } from "vitest";
import { AuthStorage } from "../src/core/auth-storage.ts";
import { ModelRuntime } from "../src/core/model-runtime.ts";
import { SettingsManager } from "../src/core/settings-manager.ts";
import { agentOf, openDurable } from "../src/experimental/durable/runtime.ts";

afterEach(() => {
	vi.restoreAllMocks();
	vi.unstubAllEnvs();
});

test("publishes newly available models after durable login without reopening the session", async () => {
	const directory = await mkdtemp(join(tmpdir(), "pi-durable-login-"));
	vi.stubEnv("PI_CODING_AGENT_DIR", join(directory, "agent"));
	const runtime = await ModelRuntime.create({
		credentials: AuthStorage.inMemory(),
		modelsPath: null,
		refreshOnCreate: false,
	});
	const faux = fauxProvider();
	runtime.registerNativeProvider({
		...faux.provider,
		auth: {
			apiKey: {
				name: "API key",
				login: async (interaction) => ({
					type: "api_key",
					key: await interaction.prompt({ type: "secret", message: "Enter API key" }),
				}),
				check: async ({ credential }) => (credential ? { type: "api_key", source: "stored" } : undefined),
				resolve: async ({ credential }) =>
					credential ? { auth: { apiKey: credential.key }, source: "stored" } : undefined,
			},
		},
	});
	await runtime.refresh({ allowNetwork: false, providers: [faux.provider.id] });
	vi.spyOn(ModelRuntime, "create").mockResolvedValue(runtime);
	vi.spyOn(SettingsManager, "create").mockReturnValue(SettingsManager.inMemory());
	try {
		const opened = await openDurable({ cwd: directory });
		try {
			expect(opened.view.current().models.some((model) => model.provider === faux.provider.id)).toBe(false);
			const cancelled = opened.controller.login(faux.provider.id, "api_key");
			await vi.waitFor(() => expect(opened.view.current().auth?.prompt).toBeDefined());
			await opened.controller.cancelLogin();
			await cancelled;
			expect(opened.view.current().auth).toBeUndefined();
			expect(opened.view.current().models.some((model) => model.provider === faux.provider.id)).toBe(false);

			const login = opened.controller.login(faux.provider.id, "api_key");
			await vi.waitFor(() => expect(opened.view.current().auth?.prompt).toBeDefined());
			await opened.controller.replyAuth(opened.view.current().auth!.prompt!.id, "test-key");
			await login;
			expect(opened.view.current().auth).toBeUndefined();
			expect(opened.view.current().notices.filter((notice) => notice.level === "error")).toEqual([]);
			const model = faux.getModel();
			expect(opened.view.current().models).toContainEqual({
				provider: model.provider,
				modelId: model.id,
				name: model.name,
				contextWindow: model.contextWindow,
			});
			await opened.controller.setModel({ provider: model.provider, modelId: model.id });
			expect(agentOf(opened.view.current().conversation).model).toEqual({
				provider: model.provider,
				modelId: model.id,
			});
		} finally {
			await opened.close();
		}
	} finally {
		await rm(directory, { recursive: true, force: true });
	}
});
