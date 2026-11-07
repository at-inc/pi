// providers/cerebras.models: the generated catalogs of the provider, read from the Rust core's catalog.

import type * as ts from "@at-inc/pi-ai/providers/cerebras.models";
import { wasm } from "@at-inc/pi-core";

export const CEREBRAS_MODELS: typeof ts.CEREBRAS_MODELS = wasm.aiBuiltinCatalog("chat", "cerebras");

export const CEREBRAS_IMAGE_MODELS: typeof ts.CEREBRAS_IMAGE_MODELS = wasm.aiBuiltinCatalog("image", "cerebras");

export const CEREBRAS_CLASSIFIER_MODELS: typeof ts.CEREBRAS_CLASSIFIER_MODELS = wasm.aiBuiltinCatalog(
	"classifier",
	"cerebras",
);
