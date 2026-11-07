// AuthProvider across the boundary: the Rust provider of an adaptOAuthProvider result travels as its handle; an
// AuthProvider written in JS is wrapped so its onUnauthorized gets the context of the TS type.

import type { AuthProviderHandle } from "@at-inc/pi-core/wasm";
import type { AuthProvider, McpFetch } from "@earendil-works/pi-mcp";
import { settle } from "./errors.ts";

const HANDLE = Symbol("pi-core AuthProviderHandle");

/** An AuthProvider that also carries its Rust provider. */
interface NativeAuthProvider extends AuthProvider {
	readonly [HANDLE]: AuthProviderHandle;
}

/** The AuthProvider of a Rust provider. */
export function nativeAuthProvider(handle: AuthProviderHandle): AuthProvider {
	const provider: NativeAuthProvider = {
		token: () => settle(handle.token()),
		[HANDLE]: handle,
	};
	if (handle.hasOnUnauthorized()) provider.onUnauthorized = (context) => settle(handle.onUnauthorized(context));
	return provider;
}

/** The Rust provider of an AuthProvider from nativeAuthProvider, passed by value. */
export function authProviderHandle(provider: AuthProvider | undefined): AuthProviderHandle | undefined {
	return (provider as Partial<NativeAuthProvider> | undefined)?.[HANDLE]?.share();
}

/** The context the core passes to a JS provider's onUnauthorized: `serverUrl` as a string, no `fetch`. */
interface CoreUnauthorizedContext {
	response: Response;
	serverUrl: string;
	token?: string;
}

/** A JS AuthProvider as the core calls it: `onUnauthorized` gets `serverUrl` as a `URL` and the transport's `fetch`. */
export function jsAuthProvider(provider: AuthProvider, fetch: McpFetch): object {
	const onUnauthorized = provider.onUnauthorized;
	return {
		token: () => provider.token(),
		...(onUnauthorized && {
			onUnauthorized: (context: CoreUnauthorizedContext) =>
				onUnauthorized.call(provider, { ...context, serverUrl: new URL(context.serverUrl), fetch }),
		}),
	};
}
