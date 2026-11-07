// images-api-registry: the registry of image-generation APIs, kept by the Rust core. A JS implementation is called as JS.

import type * as ts from "@at-inc/pi-ai/compat";
import { wasm } from "@at-inc/pi-core";
import { rethrow } from "./internal/errors.ts";

export type { ImagesApiFunction, ImagesApiProvider } from "@at-inc/pi-ai/compat";

export const registerImagesApiProvider: typeof ts.registerImagesApiProvider = (provider, sourceId) =>
	wasm.aiRegisterImagesApiProvider(provider, sourceId);

export const getImagesApiProvider: typeof ts.getImagesApiProvider = (api) => {
	const provider = wasm.aiGetImagesApiProvider(api);
	if (!provider) return undefined;
	return {
		api: provider.api,
		generateImages: (model, context, options) => provider.generateImages(model, context, options).catch(rethrow),
	};
};
