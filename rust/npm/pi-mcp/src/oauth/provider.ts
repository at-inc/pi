// oauth/provider: MemoryOAuthStateStore and McpOAuthProvider over the Rust ones. A store written in JS backs the
// Rust provider through its `load` and `save` methods.

import { wasm } from "@at-inc/pi-core";
import type { McpOAuthProviderHandle, MemoryOAuthStateStoreHandle } from "@at-inc/pi-core/wasm";
import type {
	AuthorizationServerMetadata,
	McpOAuthProviderOptions,
	McpOAuthState,
	McpOAuthStateStore,
	OAuthClientInformationMixed,
	OAuthClientMetadata,
	OAuthClientMetadataDocument,
	OAuthClientProvider,
	OAuthDiscoveryState,
	OAuthTokens,
} from "@earendil-works/pi-mcp/oauth";
import { attempt, settle } from "../internal/errors.ts";

const STORE = Symbol("pi-core MemoryOAuthStateStoreHandle");
const PROVIDER = Symbol("pi-core McpOAuthProviderHandle");

export type { McpOAuthProviderOptions, McpOAuthState, McpOAuthStateStore } from "@earendil-works/pi-mcp/oauth";

/** `MemoryOAuthStateStore`: keeps the state in memory, as copies. */
export class MemoryOAuthStateStore implements McpOAuthStateStore {
	readonly [STORE]: MemoryOAuthStateStoreHandle = wasm.mcpCreateMemoryOAuthStateStore();

	load(): McpOAuthState | undefined {
		return attempt(() => this[STORE].load());
	}

	save(state: McpOAuthState): void {
		attempt(() => this[STORE].save(state));
	}
}

/** Default stateful provider for one exact MCP server URL. Applications inject durable storage if needed. */
export class McpOAuthProvider implements OAuthClientProvider {
	readonly redirectUrl: string;
	readonly clientMetadata: OAuthClientMetadata;
	readonly clientMetadataDocument?: (
		metadata: AuthorizationServerMetadata | undefined,
	) => OAuthClientMetadataDocument | undefined;
	readonly [PROVIDER]: McpOAuthProviderHandle;

	constructor(options: McpOAuthProviderOptions) {
		const store = options.store instanceof MemoryOAuthStateStore ? options.store[STORE].share() : undefined;
		const handle = attempt(() => wasm.mcpCreateOAuthProvider(options, store, store ? undefined : options.store));
		this[PROVIDER] = handle;
		this.redirectUrl = handle.redirectUrl();
		this.clientMetadata = handle.clientMetadata();
		this.clientMetadataDocument = options.clientMetadataDocument;
	}

	state(): Promise<string> {
		return settle(this[PROVIDER].state());
	}

	clientInformation(): Promise<OAuthClientInformationMixed | undefined> {
		return settle(this[PROVIDER].clientInformation());
	}

	saveClientInformation(information: OAuthClientInformationMixed): Promise<void> {
		return settle(this[PROVIDER].saveClientInformation(information));
	}

	tokens(): Promise<OAuthTokens | undefined> {
		return settle(this[PROVIDER].tokens());
	}

	saveTokens(tokens: OAuthTokens): Promise<void> {
		return settle(this[PROVIDER].saveTokens(tokens));
	}

	redirectToAuthorization(url: URL): Promise<void> {
		return settle(this[PROVIDER].redirectToAuthorization(url));
	}

	saveCodeVerifier(verifier: string): Promise<void> {
		return settle(this[PROVIDER].saveCodeVerifier(verifier));
	}

	codeVerifier(): Promise<string> {
		return settle(this[PROVIDER].codeVerifier());
	}

	invalidateCredentials(kind: "all" | "client" | "tokens" | "verifier" | "discovery"): Promise<void> {
		return settle(this[PROVIDER].invalidateCredentials(kind));
	}

	saveDiscoveryState(discovery: OAuthDiscoveryState): Promise<void> {
		return settle(this[PROVIDER].saveDiscoveryState(discovery));
	}

	discoveryState(): Promise<OAuthDiscoveryState | undefined> {
		return settle(this[PROVIDER].discoveryState());
	}
}

/** The Rust provider of a facade `McpOAuthProvider`, passed by value; `undefined` for a provider written in JS. */
export function oauthProviderHandle(provider: OAuthClientProvider): McpOAuthProviderHandle | undefined {
	return provider instanceof McpOAuthProvider ? provider[PROVIDER].share() : undefined;
}
