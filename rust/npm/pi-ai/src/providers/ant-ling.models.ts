// providers/ant-ling.models: the generated catalogs of the provider, read from the Rust core's catalog.

import type * as ts from "@at-inc/pi-ai/providers/ant-ling.models";
import { wasm } from "@at-inc/pi-core";

export const ANT_LING_MODELS: typeof ts.ANT_LING_MODELS = wasm.aiBuiltinCatalog("chat", "ant-ling");

export const ANT_LING_IMAGE_MODELS: typeof ts.ANT_LING_IMAGE_MODELS = wasm.aiBuiltinCatalog("image", "ant-ling");

export const ANT_LING_CLASSIFIER_MODELS: typeof ts.ANT_LING_CLASSIFIER_MODELS = wasm.aiBuiltinCatalog(
	"classifier",
	"ant-ling",
);
