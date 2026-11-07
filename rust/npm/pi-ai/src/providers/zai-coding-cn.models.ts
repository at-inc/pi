// providers/zai-coding-cn.models: the generated catalogs of the provider, read from the Rust core's catalog.

import type * as ts from "@at-inc/pi-ai/providers/zai-coding-cn.models";
import { wasm } from "@at-inc/pi-core";

export const ZAI_CODING_CN_MODELS: typeof ts.ZAI_CODING_CN_MODELS = wasm.aiBuiltinCatalog("chat", "zai-coding-cn");

export const ZAI_CODING_CN_IMAGE_MODELS: typeof ts.ZAI_CODING_CN_IMAGE_MODELS = wasm.aiBuiltinCatalog(
	"image",
	"zai-coding-cn",
);

export const ZAI_CODING_CN_CLASSIFIER_MODELS: typeof ts.ZAI_CODING_CN_CLASSIFIER_MODELS = wasm.aiBuiltinCatalog(
	"classifier",
	"zai-coding-cn",
);
