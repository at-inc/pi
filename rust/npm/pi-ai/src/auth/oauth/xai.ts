// auth/oauth/xai: the xAI OAuth flow of the Rust core.

import type * as ts from "@at-inc/pi-ai/auth/oauth/xai";
import { wasm } from "@at-inc/pi-core";
import { oauthAuthOf } from "../../internal/auth.ts";

export const xaiOAuth: typeof ts.xaiOAuth = oauthAuthOf(wasm.aiXaiOAuth());
