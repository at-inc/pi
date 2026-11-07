// providers/openrouter.models: the generated catalogs of the provider, read from the Rust core's catalog.

import type * as ts from "@at-inc/pi-ai/providers/openrouter.models";
import { wasm } from "@at-inc/pi-core";

export const OPENROUTER_MODELS: typeof ts.OPENROUTER_MODELS = wasm.aiBuiltinCatalog("chat", "openrouter");

export const OPENROUTER_IMAGE_MODELS: typeof ts.OPENROUTER_IMAGE_MODELS = wasm.aiBuiltinCatalog("image", "openrouter");

export const OPENROUTER_CLASSIFIER_MODELS: typeof ts.OPENROUTER_CLASSIFIER_MODELS = wasm.aiBuiltinCatalog(
	"classifier",
	"openrouter",
);
