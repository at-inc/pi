// providers/mistral.models: the generated catalogs of the provider, read from the Rust core's catalog.

import type * as ts from "@at-inc/pi-ai/providers/mistral.models";
import { wasm } from "@at-inc/pi-core";

export const MISTRAL_MODELS: typeof ts.MISTRAL_MODELS = wasm.aiBuiltinCatalog("chat", "mistral");

export const MISTRAL_IMAGE_MODELS: typeof ts.MISTRAL_IMAGE_MODELS = wasm.aiBuiltinCatalog("image", "mistral");

export const MISTRAL_CLASSIFIER_MODELS: typeof ts.MISTRAL_CLASSIFIER_MODELS = wasm.aiBuiltinCatalog(
	"classifier",
	"mistral",
);
