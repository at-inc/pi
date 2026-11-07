// providers/xai.models: the generated catalogs of the provider, read from the Rust core's catalog.

import type * as ts from "@at-inc/pi-ai/providers/xai.models";
import { wasm } from "@at-inc/pi-core";

export const XAI_MODELS: typeof ts.XAI_MODELS = wasm.aiBuiltinCatalog("chat", "xai");

export const XAI_IMAGE_MODELS: typeof ts.XAI_IMAGE_MODELS = wasm.aiBuiltinCatalog("image", "xai");

export const XAI_CLASSIFIER_MODELS: typeof ts.XAI_CLASSIFIER_MODELS = wasm.aiBuiltinCatalog("classifier", "xai");
