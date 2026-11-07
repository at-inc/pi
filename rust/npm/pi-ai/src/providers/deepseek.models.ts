// providers/deepseek.models: the generated catalogs of the provider, read from the Rust core's catalog.

import type * as ts from "@at-inc/pi-ai/providers/deepseek.models";
import { wasm } from "@at-inc/pi-core";

export const DEEPSEEK_MODELS: typeof ts.DEEPSEEK_MODELS = wasm.aiBuiltinCatalog("chat", "deepseek");

export const DEEPSEEK_IMAGE_MODELS: typeof ts.DEEPSEEK_IMAGE_MODELS = wasm.aiBuiltinCatalog("image", "deepseek");

export const DEEPSEEK_CLASSIFIER_MODELS: typeof ts.DEEPSEEK_CLASSIFIER_MODELS = wasm.aiBuiltinCatalog(
	"classifier",
	"deepseek",
);
