// auth/oauth/openai-codex: the OpenAI Codex OAuth flow of the Rust core.

import type * as ts from "@at-inc/pi-ai/auth/oauth/openai-codex";
import { wasm } from "@at-inc/pi-core";
import { oauthAuthOf } from "../../internal/auth.ts";

export const openaiCodexOAuth: typeof ts.openaiCodexOAuth = oauthAuthOf(wasm.aiOpenaiCodexOAuth());
