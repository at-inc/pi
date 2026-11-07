// auth/oauth/anthropic: the Anthropic (Claude Pro/Max) OAuth flow of the Rust core.

import type * as ts from "@at-inc/pi-ai/auth/oauth/anthropic";
import { wasm } from "@at-inc/pi-core";
import { oauthAuthOf } from "../../internal/auth.ts";

export const anthropicOAuth: typeof ts.anthropicOAuth = oauthAuthOf(wasm.aiAnthropicOAuth());
