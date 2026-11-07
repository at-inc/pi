// providers/moonshotai-cn.models: the generated catalogs of the provider, read from the Rust core's catalog.

import type * as ts from "@at-inc/pi-ai/providers/moonshotai-cn.models";
import { wasm } from "@at-inc/pi-core";

export const MOONSHOTAI_CN_MODELS: typeof ts.MOONSHOTAI_CN_MODELS = wasm.aiBuiltinCatalog("chat", "moonshotai-cn");

export const MOONSHOTAI_CN_IMAGE_MODELS: typeof ts.MOONSHOTAI_CN_IMAGE_MODELS = wasm.aiBuiltinCatalog(
	"image",
	"moonshotai-cn",
);

export const MOONSHOTAI_CN_CLASSIFIER_MODELS: typeof ts.MOONSHOTAI_CN_CLASSIFIER_MODELS = wasm.aiBuiltinCatalog(
	"classifier",
	"moonshotai-cn",
);
