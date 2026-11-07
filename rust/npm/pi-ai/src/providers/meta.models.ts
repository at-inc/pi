// providers/meta.models: the generated catalogs of the provider, read from the Rust core's catalog.

import type * as ts from "@at-inc/pi-ai/providers/meta.models";
import { wasm } from "@at-inc/pi-core";

export const META_MODELS: typeof ts.META_MODELS = wasm.aiBuiltinCatalog("chat", "meta");

export const META_IMAGE_MODELS: typeof ts.META_IMAGE_MODELS = wasm.aiBuiltinCatalog("image", "meta");

export const META_CLASSIFIER_MODELS: typeof ts.META_CLASSIFIER_MODELS = wasm.aiBuiltinCatalog("classifier", "meta");
