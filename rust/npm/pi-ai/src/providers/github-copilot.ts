// providers/github-copilot: `githubCopilotProvider` over the Rust provider.

import type * as ts from "@at-inc/pi-ai/providers/github-copilot";
import { wasm } from "@at-inc/pi-core";
import { providerFromHandle } from "../models.ts";

export type * from "@at-inc/pi-ai/providers/github-copilot";

export const githubCopilotProvider: typeof ts.githubCopilotProvider = () =>
	providerFromHandle(wasm.aiProvider("github-copilot")) as ReturnType<typeof ts.githubCopilotProvider>;
