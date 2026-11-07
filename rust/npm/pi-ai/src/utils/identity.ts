// utils/identity: identities and headers in the Rust core. withClientIdentity is a JS wrapper of the caller's Models
// object, as in the TS: it only merges identities (in Rust) into each request's options.

import type { Models, MutableModels } from "@at-inc/pi-ai/models";
import type * as ts from "@at-inc/pi-ai/utils/identity";
import { wasm } from "@at-inc/pi-core";

export type * from "@at-inc/pi-ai/utils/identity";

export const PI_ATTRIBUTION: typeof ts.PI_ATTRIBUTION = wasm.aiPiAttribution();

export const mergeClientIdentity: typeof ts.mergeClientIdentity = (...sources) => wasm.aiMergeClientIdentity(sources);

export const getClientVersion: typeof ts.getClientVersion = (identity) => wasm.aiGetClientVersion(identity);

export const formatUserAgent: typeof ts.formatUserAgent = (identity) => wasm.aiFormatUserAgent(identity);

export const getAttributionHeaders: typeof ts.getAttributionHeaders = (target, identity) =>
	wasm.aiGetAttributionHeaders(target, identity);

export const getOpenCodeSessionHeaders: typeof ts.getOpenCodeSessionHeaders = (target, sessionId) =>
	wasm.aiGetOpenCodeSessionHeaders(target, sessionId);

export const getIdentityHeaders: typeof ts.getIdentityHeaders = (target, identity) =>
	wasm.aiGetIdentityHeaders(target, identity);

export const withClientIdentity = ((models: Models, identity: ts.ClientIdentity): Models => {
	const withIdentity = <T extends { identity?: ts.ClientIdentity }>(options: T | undefined): T =>
		({ ...options, identity: mergeClientIdentity(identity, options?.identity) }) as T;
	const wrapped: Models = {
		getProviders: () => models.getProviders(),
		getProvider: (id) => models.getProvider(id),
		getModels: (provider) => models.getModels(provider),
		getModel: (provider, id) => models.getModel(provider, id),
		getModelsOfType: (type, provider) => models.getModelsOfType(type, provider),
		getModelOfType: (type, provider, id) => models.getModelOfType(type, provider, id),
		getAllModels: (provider) => models.getAllModels(provider),
		refresh: (options) => models.refresh(options),
		checkAuth: (providerId, options) => models.checkAuth(providerId, options),
		getAvailable: (providerId, options) => models.getAvailable(providerId, options),
		getAvailableOfType: (type, providerId, options) => models.getAvailableOfType(type, providerId, options),
		getAllAvailable: (providerId, options) => models.getAllAvailable(providerId, options),
		getAuth: models.getAuth.bind(models),
		login: (providerId, type, interaction, options) => models.login(providerId, type, interaction, options),
		logout: (providerId, options) => models.logout(providerId, options),
		stream: (model, context, options) => models.stream(model, context, withIdentity(options)),
		complete: (model, context, options) => models.complete(model, context, withIdentity(options)),
		streamSimple: (model, context, options) => models.streamSimple(model, context, withIdentity(options)),
		completeSimple: (model, context, options) => models.completeSimple(model, context, withIdentity(options)),
		streamDeferred: (model, handle, options) => models.streamDeferred(model, handle, withIdentity(options)),
		fetchDeferred: (model, handle, options) => models.fetchDeferred(model, handle, withIdentity(options)),
		cancelDeferred: (model, handle, options) => models.cancelDeferred(model, handle, withIdentity(options)),
		generateImages: (model, context, options) => models.generateImages(model, context, withIdentity(options)),
		classify: (model, context, options) => models.classify(model, context, withIdentity(options)),
	};
	if (!("setProvider" in models && "deleteProvider" in models && "clearProviders" in models)) return wrapped;
	const source = models as MutableModels;
	const mutable: MutableModels = {
		...wrapped,
		setProvider: (provider) => source.setProvider(provider),
		deleteProvider: (id) => source.deleteProvider(id),
		clearProviders: () => source.clearProviders(),
	};
	return mutable;
}) as typeof ts.withClientIdentity;
