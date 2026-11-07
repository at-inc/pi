// providers/zai.models: the generated catalogs of the provider, read from the Rust core's catalog.

import type * as ts from "@at-inc/pi-ai/providers/zai.models";
import { wasm } from "@at-inc/pi-core";

export const ZAI_MODELS: typeof ts.ZAI_MODELS = wasm.aiBuiltinCatalog("chat", "zai");

export const ZAI_IMAGE_MODELS: typeof ts.ZAI_IMAGE_MODELS = wasm.aiBuiltinCatalog("image", "zai");

export const ZAI_CLASSIFIER_MODELS: typeof ts.ZAI_CLASSIFIER_MODELS = wasm.aiBuiltinCatalog("classifier", "zai");
