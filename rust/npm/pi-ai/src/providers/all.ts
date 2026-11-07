// providers/all: the built-in catalog reads, the built-in providers and a Models collection of them, over the Rust core.

import type * as ts from "@at-inc/pi-ai/providers/all";
import { wasm } from "@at-inc/pi-core";
import { providerFromHandle, RustModels } from "../models.ts";

export type * from "@at-inc/pi-ai/providers/all";
export { radiusProvider } from "./radius.ts";

export const getBuiltinModel: typeof ts.getBuiltinModel = (provider, modelId) =>
	wasm.aiGetBuiltinModelOfType("chat", provider, modelId as string);

export const getBuiltinImageModel: typeof ts.getBuiltinImageModel = (provider, modelId) =>
	wasm.aiGetBuiltinModelOfType("image", provider, modelId as string);

export const getBuiltinClassifierModel: typeof ts.getBuiltinClassifierModel = (provider, modelId) =>
	wasm.aiGetBuiltinModelOfType("classifier", provider, modelId as string);

export const getBuiltinProviders: typeof ts.getBuiltinProviders = () =>
	wasm.aiGetBuiltinProviders() as ReturnType<typeof ts.getBuiltinProviders>;

export const getBuiltinModelDataGeneratedAt: typeof ts.getBuiltinModelDataGeneratedAt = () =>
	wasm.aiGetBuiltinModelDataGeneratedAt() ?? Number.NaN;

export const getBuiltinModels: typeof ts.getBuiltinModels = (provider) =>
	wasm.aiGetBuiltinModelsOfType("chat", provider);

export const getBuiltinImageModels: typeof ts.getBuiltinImageModels = (provider) =>
	wasm.aiGetBuiltinModelsOfType("image", provider);

export const getBuiltinClassifierModels: typeof ts.getBuiltinClassifierModels = (provider) =>
	wasm.aiGetBuiltinModelsOfType("classifier", provider);

export const getAllBuiltinModels: typeof ts.getAllBuiltinModels = (provider) => wasm.aiGetAllBuiltinModels(provider);

export const builtinProviders: typeof ts.builtinProviders = () =>
	wasm.aiBuiltinProviders().map((handle) => providerFromHandle(handle));

export const builtinModels: typeof ts.builtinModels = (options) => new RustModels(wasm.aiBuiltinModels(options ?? {}));

export const builtinImagesProviders: typeof ts.builtinImagesProviders = () =>
	wasm.aiBuiltinImagesProviders().map((handle) => providerFromHandle(handle));
