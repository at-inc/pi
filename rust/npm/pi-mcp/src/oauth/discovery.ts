// oauth/discovery: OAuth metadata discovery over the Rust functions. The core returns URLs as strings; the values
// the TS types as `URL` become `URL` objects here.

import { wasm } from "@at-inc/pi-core";
import type * as ts from "@earendil-works/pi-mcp/oauth";
import { attempt, settle } from "../internal/errors.ts";

export const parseWwwAuthenticate: typeof ts.parseWwwAuthenticate = (header) => {
	const challenge = attempt(() => wasm.mcpParseWwwAuthenticate(header));
	if (challenge.resourceMetadataUrl !== undefined)
		challenge.resourceMetadataUrl = new URL(challenge.resourceMetadataUrl);
	return challenge;
};

export const discoverProtectedResourceMetadata: typeof ts.discoverProtectedResourceMetadata = (
	serverUrl,
	options = {},
) => settle(wasm.mcpDiscoverProtectedResourceMetadata(serverUrl, options));

export const buildAuthorizationServerDiscoveryUrls: typeof ts.buildAuthorizationServerDiscoveryUrls = (
	authorizationServerUrl,
) =>
	attempt(() => wasm.mcpBuildAuthorizationServerDiscoveryUrls(authorizationServerUrl)).map(
		(entry: { url: string; type: "oauth" | "oidc" }) => ({ url: new URL(entry.url), type: entry.type }),
	);

export const discoverAuthorizationServerMetadata: typeof ts.discoverAuthorizationServerMetadata = (
	authorizationServerUrl,
	options = {},
) => settle(wasm.mcpDiscoverAuthorizationServerMetadata(authorizationServerUrl, options));

export const discoverOAuthServerInfo: typeof ts.discoverOAuthServerInfo = (serverUrl, options = {}) =>
	settle(wasm.mcpDiscoverOAuthServerInfo(serverUrl, options));

export const resourceUrlFromServerUrl: typeof ts.resourceUrlFromServerUrl = (value) =>
	new URL(attempt(() => wasm.mcpResourceUrlFromServerUrl(value)));

export const selectResource: typeof ts.selectResource = (serverUrl, metadata) =>
	attempt(() => wasm.mcpSelectResource(serverUrl, metadata));
