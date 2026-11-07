// providers/anthropic.models: the generated catalogs of the provider, read from the Rust core's catalog.

import type * as ts from "@at-inc/pi-ai/providers/anthropic.models";
import { wasm } from "@at-inc/pi-core";

export const ANTHROPIC_MODELS: typeof ts.ANTHROPIC_MODELS = wasm.aiBuiltinCatalog("chat", "anthropic");

export const ANTHROPIC_IMAGE_MODELS: typeof ts.ANTHROPIC_IMAGE_MODELS = wasm.aiBuiltinCatalog("image", "anthropic");

export const ANTHROPIC_CLASSIFIER_MODELS: typeof ts.ANTHROPIC_CLASSIFIER_MODELS = wasm.aiBuiltinCatalog(
	"classifier",
	"anthropic",
);
