// providers/radius.models: the generated catalogs of the provider, read from the Rust core's catalog.

import type * as ts from "@at-inc/pi-ai/providers/radius.models";
import { wasm } from "@at-inc/pi-core";

export const RADIUS_MODELS: typeof ts.RADIUS_MODELS = wasm.aiBuiltinCatalog("chat", "radius");

export const RADIUS_IMAGE_MODELS: typeof ts.RADIUS_IMAGE_MODELS = wasm.aiBuiltinCatalog("image", "radius");

export const RADIUS_CLASSIFIER_MODELS: typeof ts.RADIUS_CLASSIFIER_MODELS = wasm.aiBuiltinCatalog(
	"classifier",
	"radius",
);
