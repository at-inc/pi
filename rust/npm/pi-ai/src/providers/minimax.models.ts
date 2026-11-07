// providers/minimax.models: the generated catalogs of the provider, read from the Rust core's catalog.

import type * as ts from "@at-inc/pi-ai/providers/minimax.models";
import { wasm } from "@at-inc/pi-core";

export const MINIMAX_MODELS: typeof ts.MINIMAX_MODELS = wasm.aiBuiltinCatalog("chat", "minimax");

export const MINIMAX_IMAGE_MODELS: typeof ts.MINIMAX_IMAGE_MODELS = wasm.aiBuiltinCatalog("image", "minimax");

export const MINIMAX_CLASSIFIER_MODELS: typeof ts.MINIMAX_CLASSIFIER_MODELS = wasm.aiBuiltinCatalog(
	"classifier",
	"minimax",
);
