// providers/minimax-cn.models: the generated catalogs of the provider, read from the Rust core's catalog.

import type * as ts from "@at-inc/pi-ai/providers/minimax-cn.models";
import { wasm } from "@at-inc/pi-core";

export const MINIMAX_CN_MODELS: typeof ts.MINIMAX_CN_MODELS = wasm.aiBuiltinCatalog("chat", "minimax-cn");

export const MINIMAX_CN_IMAGE_MODELS: typeof ts.MINIMAX_CN_IMAGE_MODELS = wasm.aiBuiltinCatalog("image", "minimax-cn");

export const MINIMAX_CN_CLASSIFIER_MODELS: typeof ts.MINIMAX_CN_CLASSIFIER_MODELS = wasm.aiBuiltinCatalog(
	"classifier",
	"minimax-cn",
);
