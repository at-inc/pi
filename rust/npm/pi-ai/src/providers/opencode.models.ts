// providers/opencode.models: the generated catalogs of the provider, read from the Rust core's catalog.

import type * as ts from "@at-inc/pi-ai/providers/opencode.models";
import { wasm } from "@at-inc/pi-core";

export const OPENCODE_MODELS: typeof ts.OPENCODE_MODELS = wasm.aiBuiltinCatalog("chat", "opencode");

export const OPENCODE_IMAGE_MODELS: typeof ts.OPENCODE_IMAGE_MODELS = wasm.aiBuiltinCatalog("image", "opencode");

export const OPENCODE_CLASSIFIER_MODELS: typeof ts.OPENCODE_CLASSIFIER_MODELS = wasm.aiBuiltinCatalog(
	"classifier",
	"opencode",
);
