// image-models: the deprecated built-in image catalog reads, over the Rust core.

import type * as ts from "@at-inc/pi-ai/compat";
import { wasm } from "@at-inc/pi-core";

export const getImageModel: typeof ts.getImageModel = (provider, modelId) =>
	wasm.aiGetImageModel(provider, modelId as string);

export const getImageProviders: typeof ts.getImageProviders = () =>
	wasm.aiGetImageProviders() as ReturnType<typeof ts.getImageProviders>;

export const getImageModels: typeof ts.getImageModels = (provider) => wasm.aiGetImageModels(provider);
