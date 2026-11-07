// providers/openai-codex: `openaiCodexProvider` over the Rust provider.

import type * as ts from "@at-inc/pi-ai/providers/openai-codex";
import { wasm } from "@at-inc/pi-core";
import { providerFromHandle } from "../models.ts";

export type * from "@at-inc/pi-ai/providers/openai-codex";

export const openaiCodexProvider: typeof ts.openaiCodexProvider = () =>
	providerFromHandle(wasm.aiProvider("openai-codex")) as ReturnType<typeof ts.openaiCodexProvider>;

export const OPENAI_CODEX_BASE_URL: typeof ts.OPENAI_CODEX_BASE_URL = "https://chatgpt.com/backend-api";
