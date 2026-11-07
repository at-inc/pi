// auth/oauth/kimi-coding: the Kimi Coding OAuth flow of the Rust core.

import type * as ts from "@at-inc/pi-ai/auth/oauth/kimi-coding";
import { wasm } from "@at-inc/pi-core";
import { oauthAuthOf } from "../../internal/auth.ts";

export const kimiCodingOAuth: typeof ts.kimiCodingOAuth = oauthAuthOf(wasm.aiKimiCodingOAuth());
