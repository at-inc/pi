// oauth/flow: authorizeMcp, adaptOAuthProvider and the flow steps over the Rust flow. A McpOAuthProvider of the
// facade is passed by its Rust handle; any other OAuthClientProvider is called through its methods.

import { wasm } from "@at-inc/pi-core";
import type * as ts from "@earendil-works/pi-mcp/oauth";
import { nativeAuthProvider } from "../internal/auth-provider.ts";
import { attempt, settle } from "../internal/errors.ts";
import { oauthProviderHandle } from "./provider.ts";

export type {
	AddClientAuthentication,
	OAuthClientMetadataDocument,
	OAuthClientProvider,
	OAuthFlowOptions,
	OAuthFlowResult,
	TokenRequestOptions,
} from "@earendil-works/pi-mcp/oauth";

export const startAuthorization: typeof ts.startAuthorization = async (authorizationServerUrl, options) => {
	const start = await settle(wasm.mcpStartAuthorization(authorizationServerUrl, options));
	return { authorizationUrl: new URL(start.authorizationUrl), codeVerifier: start.codeVerifier };
};

export const registerClient: typeof ts.registerClient = (authorizationServerUrl, options) =>
	settle(wasm.mcpRegisterClient(authorizationServerUrl, options));

export const exchangeAuthorizationCode: typeof ts.exchangeAuthorizationCode = (authorizationServerUrl, options) =>
	settle(wasm.mcpExchangeAuthorizationCode(authorizationServerUrl, options));

export const refreshAuthorization: typeof ts.refreshAuthorization = (authorizationServerUrl, options) =>
	settle(wasm.mcpRefreshAuthorization(authorizationServerUrl, options));

export const stepUpScope: typeof ts.stepUpScope = (granted, challenged) => wasm.mcpStepUpScope(granted, challenged);

export const authorizeMcp: typeof ts.authorizeMcp = (provider, options) => {
	const native = oauthProviderHandle(provider);
	return settle(wasm.mcpAuthorizeMcp(native, native ? undefined : provider, options));
};

export const adaptOAuthProvider: typeof ts.adaptOAuthProvider = (provider) => {
	const native = oauthProviderHandle(provider);
	return nativeAuthProvider(attempt(() => wasm.mcpAdaptOAuthProvider(native, native ? undefined : provider)));
};
