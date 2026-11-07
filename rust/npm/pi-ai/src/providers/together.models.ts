// providers/together.models: the generated catalogs of the provider, read from the Rust core's catalog.

import type * as ts from "@at-inc/pi-ai/providers/together.models";
import { wasm } from "@at-inc/pi-core";

export const TOGETHER_MODELS: typeof ts.TOGETHER_MODELS = wasm.aiBuiltinCatalog("chat", "together");

export const TOGETHER_IMAGE_MODELS: typeof ts.TOGETHER_IMAGE_MODELS = wasm.aiBuiltinCatalog("image", "together");

export const TOGETHER_CLASSIFIER_MODELS: typeof ts.TOGETHER_CLASSIFIER_MODELS = wasm.aiBuiltinCatalog(
	"classifier",
	"together",
);
