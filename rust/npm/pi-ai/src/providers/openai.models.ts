// providers/openai.models: the generated catalogs of the provider, read from the Rust core's catalog.

import type * as ts from "@at-inc/pi-ai/providers/openai.models";
import { wasm } from "@at-inc/pi-core";

export const OPENAI_MODELS: typeof ts.OPENAI_MODELS = wasm.aiBuiltinCatalog("chat", "openai");

export const OPENAI_IMAGE_MODELS: typeof ts.OPENAI_IMAGE_MODELS = wasm.aiBuiltinCatalog("image", "openai");

export const OPENAI_CLASSIFIER_MODELS: typeof ts.OPENAI_CLASSIFIER_MODELS = wasm.aiBuiltinCatalog(
	"classifier",
	"openai",
);
