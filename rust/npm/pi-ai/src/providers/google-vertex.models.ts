// providers/google-vertex.models: the generated catalogs of the provider, read from the Rust core's catalog.

import type * as ts from "@at-inc/pi-ai/providers/google-vertex.models";
import { wasm } from "@at-inc/pi-core";

export const GOOGLE_VERTEX_MODELS: typeof ts.GOOGLE_VERTEX_MODELS = wasm.aiBuiltinCatalog("chat", "google-vertex");

export const GOOGLE_VERTEX_IMAGE_MODELS: typeof ts.GOOGLE_VERTEX_IMAGE_MODELS = wasm.aiBuiltinCatalog(
	"image",
	"google-vertex",
);

export const GOOGLE_VERTEX_CLASSIFIER_MODELS: typeof ts.GOOGLE_VERTEX_CLASSIFIER_MODELS = wasm.aiBuiltinCatalog(
	"classifier",
	"google-vertex",
);
