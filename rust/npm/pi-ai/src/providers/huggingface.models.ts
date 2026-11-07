// providers/huggingface.models: the generated catalogs of the provider, read from the Rust core's catalog.

import type * as ts from "@at-inc/pi-ai/providers/huggingface.models";
import { wasm } from "@at-inc/pi-core";

export const HUGGINGFACE_MODELS: typeof ts.HUGGINGFACE_MODELS = wasm.aiBuiltinCatalog("chat", "huggingface");

export const HUGGINGFACE_IMAGE_MODELS: typeof ts.HUGGINGFACE_IMAGE_MODELS = wasm.aiBuiltinCatalog(
	"image",
	"huggingface",
);

export const HUGGINGFACE_CLASSIFIER_MODELS: typeof ts.HUGGINGFACE_CLASSIFIER_MODELS = wasm.aiBuiltinCatalog(
	"classifier",
	"huggingface",
);
