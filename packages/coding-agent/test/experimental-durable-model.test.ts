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

test.each([
	{ saved: false, codexAvailable: true, expected: "openai-codex" },
	{ saved: true, codexAvailable: true, expected: "anthropic" },
	{ saved: false, codexAvailable: false, expected: "anthropic" },
])("selects $expected with saved=$saved and codexAvailable=$codexAvailable", async (scenario) => {
	const directory = await mkdtemp(join(tmpdir(), "pi-durable-model-"));
	vi.stubEnv("PI_CODING_AGENT_DIR", join(directory, "agent"));
	const runtime = await ModelRuntime.create({
		credentials: AuthStorage.inMemory(),
		modelsPath: null,
		refreshOnCreate: false,
	});
	for (const [provider, id] of [
		["anthropic", "claude-opus-4-8"],
		["openai-codex", "gpt-6-sol"],
	]) {
		const faux = fauxProvider({ provider, models: [{ id, reasoning: true }] });
		const available = provider !== "openai-codex" || scenario.codexAvailable;
		runtime.registerNativeProvider({
			...faux.provider,
			auth: {
				apiKey: {
					name: "Test access",
					check: async () => (available ? { type: "api_key" } : undefined),
					resolve: async () => (available ? { auth: {} } : undefined),
				},
			},
		});
	}
	await runtime.refresh({ allowNetwork: false, providers: ["anthropic", "openai-codex"] });
	vi.spyOn(ModelRuntime, "create").mockResolvedValue(runtime);
	vi.spyOn(SettingsManager, "create").mockReturnValue(
		SettingsManager.inMemory({
			...(scenario.saved ? { defaultProvider: "anthropic", defaultModel: "claude-opus-4-8" } : {}),
			defaultThinkingLevel: "high",
		}),
	);
	try {
		const opened = await openDurable({ cwd: directory });
		try {
			expect(agentOf(opened.view.current().conversation).model?.provider).toBe(scenario.expected);
			if (scenario.codexAvailable) {
				expect(agentOf(opened.view.current().conversation).thinkingLevel).toBe("high");
			}
			await opened.controller.setModel({ provider: "anthropic", modelId: "claude-opus-4-8" });
		} finally {
			await opened.close();
		}
		const resumed = await openDurable({ cwd: directory, continueSession: true });
		try {
			expect(agentOf(resumed.view.current().conversation).model).toEqual({
				provider: "anthropic",
				modelId: "claude-opus-4-8",
			});
		} finally {
			await resumed.close();
		}
	} finally {
		await rm(directory, { recursive: true, force: true });
	}
});
