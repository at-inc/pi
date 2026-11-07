// auth/helpers: envApiKeyAuth and lazyOAuth, Rust auth values as TS auth objects.

import type * as ts from "@at-inc/pi-ai";
import { wasm } from "@at-inc/pi-core";
import { apiKeyAuthOf, oauthAuthOf } from "../internal/auth.ts";

export const envApiKeyAuth: typeof ts.envApiKeyAuth = (name, envVars) =>
	apiKeyAuthOf(wasm.aiEnvApiKeyAuth(name, [...envVars]));

export const lazyOAuth: typeof ts.lazyOAuth = (input) => {
	const auth = oauthAuthOf(wasm.aiLazyOAuth(input));
	// TS copies the fields as given, `undefined` included.
	return { ...auth, isSubscription: input.isSubscription, loginLabel: input.loginLabel };
};
