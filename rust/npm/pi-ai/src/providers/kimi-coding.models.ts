// providers/kimi-coding.models: the generated catalogs of the provider, read from the Rust core's catalog.

import type * as ts from "@at-inc/pi-ai/providers/kimi-coding.models";
import { wasm } from "@at-inc/pi-core";

export const KIMI_CODING_MODELS: typeof ts.KIMI_CODING_MODELS = wasm.aiBuiltinCatalog("chat", "kimi-coding");

export const KIMI_CODING_IMAGE_MODELS: typeof ts.KIMI_CODING_IMAGE_MODELS = wasm.aiBuiltinCatalog(
	"image",
	"kimi-coding",
);

export const KIMI_CODING_CLASSIFIER_MODELS: typeof ts.KIMI_CODING_CLASSIFIER_MODELS = wasm.aiBuiltinCatalog(
	"classifier",
	"kimi-coding",
);
