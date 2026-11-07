// providers/baseten.models: the generated catalogs of the provider, read from the Rust core's catalog.

import type * as ts from "@at-inc/pi-ai/providers/baseten.models";
import { wasm } from "@at-inc/pi-core";

export const BASETEN_MODELS: typeof ts.BASETEN_MODELS = wasm.aiBuiltinCatalog("chat", "baseten");

export const BASETEN_IMAGE_MODELS: typeof ts.BASETEN_IMAGE_MODELS = wasm.aiBuiltinCatalog("image", "baseten");

export const BASETEN_CLASSIFIER_MODELS: typeof ts.BASETEN_CLASSIFIER_MODELS = wasm.aiBuiltinCatalog(
	"classifier",
	"baseten",
);
