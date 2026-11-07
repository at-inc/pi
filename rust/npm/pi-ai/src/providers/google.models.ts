// providers/google.models: the generated catalogs of the provider, read from the Rust core's catalog.

import type * as ts from "@at-inc/pi-ai/providers/google.models";
import { wasm } from "@at-inc/pi-core";

export const GOOGLE_MODELS: typeof ts.GOOGLE_MODELS = wasm.aiBuiltinCatalog("chat", "google");

export const GOOGLE_IMAGE_MODELS: typeof ts.GOOGLE_IMAGE_MODELS = wasm.aiBuiltinCatalog("image", "google");

export const GOOGLE_CLASSIFIER_MODELS: typeof ts.GOOGLE_CLASSIFIER_MODELS = wasm.aiBuiltinCatalog(
	"classifier",
	"google",
);
