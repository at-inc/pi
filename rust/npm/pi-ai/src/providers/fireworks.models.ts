// providers/fireworks.models: the generated catalogs of the provider, read from the Rust core's catalog.

import type * as ts from "@at-inc/pi-ai/providers/fireworks.models";
import { wasm } from "@at-inc/pi-core";

export const FIREWORKS_MODELS: typeof ts.FIREWORKS_MODELS = wasm.aiBuiltinCatalog("chat", "fireworks");

export const FIREWORKS_IMAGE_MODELS: typeof ts.FIREWORKS_IMAGE_MODELS = wasm.aiBuiltinCatalog("image", "fireworks");

export const FIREWORKS_CLASSIFIER_MODELS: typeof ts.FIREWORKS_CLASSIFIER_MODELS = wasm.aiBuiltinCatalog(
	"classifier",
	"fireworks",
);
