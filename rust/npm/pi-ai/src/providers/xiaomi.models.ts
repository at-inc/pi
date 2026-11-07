// providers/xiaomi.models: the generated catalogs of the provider, read from the Rust core's catalog.

import type * as ts from "@at-inc/pi-ai/providers/xiaomi.models";
import { wasm } from "@at-inc/pi-core";

export const XIAOMI_MODELS: typeof ts.XIAOMI_MODELS = wasm.aiBuiltinCatalog("chat", "xiaomi");

export const XIAOMI_IMAGE_MODELS: typeof ts.XIAOMI_IMAGE_MODELS = wasm.aiBuiltinCatalog("image", "xiaomi");

export const XIAOMI_CLASSIFIER_MODELS: typeof ts.XIAOMI_CLASSIFIER_MODELS = wasm.aiBuiltinCatalog(
	"classifier",
	"xiaomi",
);
