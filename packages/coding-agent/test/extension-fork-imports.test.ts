import { mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import * as agentCore from "@at-inc/pi-agent-core";
import * as aiCompat from "@at-inc/pi-ai/compat";
import * as aiModels from "@at-inc/pi-ai/models";
import * as aiOAuth from "@at-inc/pi-ai/oauth";
import * as aiProviders from "@at-inc/pi-ai/providers/all";
import { describe, expect, it } from "vitest";
import { loadExtensions } from "../src/core/extensions/loader.ts";
import { VIRTUAL_MODULES } from "../src/core/extensions/virtual-modules.ts";

describe("fork extension imports", () => {
	it.each([
		["@at-inc/pi-agent-core", "@mariozechner/pi-agent-core", agentCore],
		["@at-inc/pi-ai", "@mariozechner/pi-ai", aiCompat],
		["@at-inc/pi-ai/compat", "@mariozechner/pi-ai/compat", aiCompat],
		["@at-inc/pi-ai/models", "@mariozechner/pi-ai/models", aiModels],
		["@at-inc/pi-ai/oauth", "@mariozechner/pi-ai/oauth", aiOAuth],
		["@at-inc/pi-ai/providers/all", "@mariozechner/pi-ai/providers/all", aiProviders],
	] as const)("maps %s and %s to the fork module", (specifier, legacySpecifier, module) => {
		expect(VIRTUAL_MODULES[specifier]).toBe(module);
		expect(VIRTUAL_MODULES[legacySpecifier]).toBe(module);
	});

	it("keeps the fork coding-agent alias", () => {
		expect(VIRTUAL_MODULES["@at-inc/pi"]).toBeDefined();
		expect(VIRTUAL_MODULES["@at-inc/pi"]).toBe(VIRTUAL_MODULES["@mariozechner/pi-coding-agent"]);
	});

	it("loads extensions importing fork models alongside the legacy aliases", async () => {
		const dir = mkdtempSync(join(tmpdir(), "pi-fork-imports-"));
		try {
			const extensionPath = join(dir, "extension.ts");
			writeFileSync(
				extensionPath,
				`
				import { getAgentDir } from "@at-inc/pi";
				import { Agent } from "@at-inc/pi-agent-core";
				import { Agent as LegacyAgent } from "@mariozechner/pi-agent-core";
				import { getModel } from "@at-inc/pi-ai";
				import { getModel as getCompatModel } from "@at-inc/pi-ai/compat";
				import { clampThinkingLevel } from "@at-inc/pi-ai/models";
				import { clampThinkingLevel as legacyClampThinkingLevel } from "@mariozechner/pi-ai/models";
				import * as oauth from "@at-inc/pi-ai/oauth";
				import { builtinProviders } from "@at-inc/pi-ai/providers/all";
				export default function(pi) {
					if (Agent !== LegacyAgent || getModel !== getCompatModel || clampThinkingLevel !== legacyClampThinkingLevel) {
						throw new Error("Fork aliases resolved to different modules");
					}
					if (typeof getAgentDir !== "function" || typeof clampThinkingLevel !== "function" || !builtinProviders || !oauth) {
						throw new Error("Fork imports are missing");
					}
					pi.registerCommand("fork-imports", { handler: async () => {} });
				}
				`,
			);

			const result = await loadExtensions([extensionPath], dir);
			expect(result.errors).toEqual([]);
			expect(result.extensions).toHaveLength(1);
			expect(result.extensions[0].commands.has("fork-imports")).toBe(true);
			result.runtime.invalidate();
		} finally {
			rmSync(dir, { recursive: true, force: true });
		}
	});
});
