// providers/azure.models: the generated catalogs of the provider, read from the Rust core's catalog.

import type * as ts from "@at-inc/pi-ai/providers/azure.models";
import { wasm } from "@at-inc/pi-core";

export const AZURE_MODELS: typeof ts.AZURE_MODELS = wasm.aiBuiltinCatalog("chat", "azure");

export const AZURE_IMAGE_MODELS: typeof ts.AZURE_IMAGE_MODELS = wasm.aiBuiltinCatalog("image", "azure");

export const AZURE_CLASSIFIER_MODELS: typeof ts.AZURE_CLASSIFIER_MODELS = wasm.aiBuiltinCatalog("classifier", "azure");
