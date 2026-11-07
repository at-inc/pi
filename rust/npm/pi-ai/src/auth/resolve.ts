// auth/resolve: provider auth resolution and the locked OAuth refresh over JS stores and auth objects.
// `ModelsError` belongs to the models slice (utils/models-error) and is not re-exported here yet.

import type * as ts from "@at-inc/pi-ai/auth/resolve";
import { wasm } from "@at-inc/pi-core";

export type * from "@at-inc/pi-ai/auth/resolve";

export const resolveProviderAuth: typeof ts.resolveProviderAuth = async (
	provider,
	credentials,
	authContext,
	overrides,
) => wasm.aiResolveProviderAuth(provider, credentials, authContext, overrides);

export const refreshStoredOAuthCredential: typeof ts.refreshStoredOAuthCredential = (
	credentials,
	providerId,
	oauth,
	needsRefresh,
	signal,
) => wasm.aiRefreshStoredOAuthCredential(credentials, providerId, oauth, needsRefresh, signal);
