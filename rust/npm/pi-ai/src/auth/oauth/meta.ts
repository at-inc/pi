// auth/oauth/meta: the Meta OAuth flow of the Rust core.

import type * as ts from "@at-inc/pi-ai/auth/oauth/meta";
import { wasm } from "@at-inc/pi-core";
import { oauthAuthOf } from "../../internal/auth.ts";

export const metaOAuth: typeof ts.metaOAuth = oauthAuthOf(wasm.aiMetaOAuth());
