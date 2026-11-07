// auth/oauth/radius: the Radius gateway OAuth flow of the Rust core.

import type * as ts from "@at-inc/pi-ai/auth/oauth/radius";
import { wasm } from "@at-inc/pi-core";
import { oauthAuthOf } from "../../internal/auth.ts";

export type * from "@at-inc/pi-ai/auth/oauth/radius";

export const createRadiusOAuth: typeof ts.createRadiusOAuth = (options) =>
	oauthAuthOf(wasm.aiCreateRadiusOAuth(options));
