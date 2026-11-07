// providers/moonshotai.models: the generated catalogs of the provider, read from the Rust core's catalog.

import type * as ts from "@at-inc/pi-ai/providers/moonshotai.models";
import { wasm } from "@at-inc/pi-core";

export const MOONSHOTAI_MODELS: typeof ts.MOONSHOTAI_MODELS = wasm.aiBuiltinCatalog("chat", "moonshotai");

export const MOONSHOTAI_IMAGE_MODELS: typeof ts.MOONSHOTAI_IMAGE_MODELS = wasm.aiBuiltinCatalog("image", "moonshotai");

export const MOONSHOTAI_CLASSIFIER_MODELS: typeof ts.MOONSHOTAI_CLASSIFIER_MODELS = wasm.aiBuiltinCatalog(
	"classifier",
	"moonshotai",
);
