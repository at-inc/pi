// The JS objects of Rust auth values: ApiKeyAuth, OAuthAuth and AuthContext objects of the TS shape whose methods
// call their wasm handles. Passed back into the core, they are called like any JS auth object.

import type { ApiKeyAuth, AuthContext, OAuthAuth } from "@at-inc/pi-ai";
import type { ApiKeyAuthHandle, AuthContextHandle, OAuthAuthHandle } from "@at-inc/pi-core/wasm";

/** The ApiKeyAuth object of a Rust ApiKeyAuth: `login` and `check` are present when the Rust auth has them. */
export function apiKeyAuthOf(handle: ApiKeyAuthHandle): ApiKeyAuth {
	return {
		name: handle.name,
		...(handle.hasLogin ? { login: (interaction) => handle.login(interaction) } : {}),
		...(handle.hasCheck ? { check: (input) => handle.check(input) } : {}),
		resolve: (input) => handle.resolve(input),
	};
}

/** The OAuthAuth object of a Rust OAuthAuth. */
export function oauthAuthOf(handle: OAuthAuthHandle): OAuthAuth {
	const auth: OAuthAuth = {
		name: handle.name,
		login: (interaction, options) => handle.login(interaction, options),
		refresh: (credential, signal) => handle.refresh(credential, signal),
		toAuth: (credential) => handle.toAuth(credential),
	};
	if (handle.isSubscription !== undefined) auth.isSubscription = handle.isSubscription;
	if (handle.loginLabel !== undefined) auth.loginLabel = handle.loginLabel;
	return auth;
}

/** The AuthContext object of a Rust AuthContext. */
export function authContextOf(handle: AuthContextHandle): AuthContext {
	return { env: (name) => handle.env(name), fileExists: (path) => handle.fileExists(path) };
}
