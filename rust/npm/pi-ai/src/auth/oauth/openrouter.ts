// auth/oauth/openrouter: the OpenRouter OAuth flow of the Rust core.

import type * as ts from "@at-inc/pi-ai/auth/oauth/openrouter";
import { wasm } from "@at-inc/pi-core";
import { oauthAuthOf } from "../../internal/auth.ts";

export const openRouterOAuth: typeof ts.openRouterOAuth = oauthAuthOf(wasm.aiOpenRouterOAuth());
